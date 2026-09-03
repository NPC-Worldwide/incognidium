//! Web font (@font-face) loading.
//!
//! The CSS parser collects `@font-face` rules into `Stylesheet::font_faces`,
//! but the raw font files themselves are not fetched during parsing. This
//! module downloads and registers them so text measurement and painting can
//! use the fonts a page actually declares instead of the built-in fallback.
//!
//! Supported sources: `data:` URIs, and http(s)/file URLs pointing at bare
//! sfnt data (TTF/OTF, signatures `0x00010000`, `true`, `OTTO`, `ttcf`), WOFF
//! 1.0 containers (per-table zlib), or WOFF 2.0 containers (one brotli
//! stream). WOFF 2.0 table transforms (compressed glyf/loca/hmtx encodings)
//! are not implemented; a face that only offers a transformed container is
//! skipped and the engine falls back as if the face did not exist.

use crate::Stylesheet;
use std::sync::{Arc, Mutex, OnceLock};

/// Upper bound on faces registered per document. Pages sometimes ship a
/// hundred faces across many subsets; every registered face costs a network
/// fetch, so keep the working set small.
const MAX_FACES: usize = 24;

/// Upper bound on a single font file (some display faces are multi-MB).
const MAX_FONT_BYTES: usize = 6 * 1024 * 1024;

/// One registered face: family (lowercased), weight, style, and the decoded
/// sfnt bytes ready for `fontdue::Font::from_bytes`.
struct RegisteredFace {
    family: String,
    weight: u16,
    italic: bool,
    data: Arc<Vec<u8>>,
}

fn registry() -> &'static Mutex<Vec<RegisteredFace>> {
    static REGISTRY: OnceLock<Mutex<Vec<RegisteredFace>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}

/// Remove all registered faces. Call before loading a new document so fonts
/// from a previous page do not linger.
pub fn clear() {
    if let Ok(mut r) = registry().lock() {
        r.clear();
    }
}

/// Register a decoded face. `family` is matched case-insensitively; quotes
/// are stripped by the caller. Multiple faces per family (different weights
/// or styles) are all kept.
pub fn register_face(family: &str, weight: u16, italic: bool, data: Vec<u8>) {
    if family.is_empty() || data.is_empty() {
        return;
    }
    let mut reg = match registry().lock() {
        Ok(r) => r,
        Err(_) => return,
    };
    if reg.len() >= MAX_FACES {
        return;
    }
    let family = family.to_lowercase();
    if reg
        .iter()
        .any(|f| f.family == family && f.weight == weight && f.italic == italic)
    {
        return;
    }
    reg.push(RegisteredFace {
        family,
        weight,
        italic,
        data: Arc::new(data),
    });
}

/// Look up the best face for a family/weight/style combination. Prefers an
/// exact style match, then the closest weight within the matching style, then
/// any style in the family.
pub fn lookup(family: &str, weight: u16, italic: bool) -> Option<Arc<Vec<u8>>> {
    let reg = registry().lock().ok()?;
    let family = family.to_lowercase();
    let faces: Vec<&RegisteredFace> = reg.iter().filter(|f| f.family == family).collect();
    if faces.is_empty() {
        return None;
    }
    let exact_style: Vec<&RegisteredFace> = faces
        .iter()
        .copied()
        .filter(|f| f.italic == italic)
        .collect();
    let pool: Vec<&RegisteredFace> = if exact_style.is_empty() {
        faces
    } else {
        exact_style
    };
    pool.into_iter()
        .min_by_key(|f| (f.weight as i32 - weight as i32).abs())
        .map(|f| Arc::clone(&f.data))
}

/// True when at least one face is registered for the family.
pub fn has_family(family: &str) -> bool {
    let family = family.to_lowercase();
    registry()
        .lock()
        .map(|r| r.iter().any(|f| f.family == family))
        .unwrap_or(false)
}

/// Parse a `font-weight` descriptor value ("normal", "bold", "400", "700").
fn parse_weight(value: &str) -> u16 {
    let v = value.trim();
    match v {
        "normal" => 400,
        "bold" => 700,
        _ => v
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u16>().ok())
            .filter(|w| (1..=1000).contains(w))
            .unwrap_or(400),
    }
}

/// Parse a `font-style` descriptor value.
fn parse_italic(value: &str) -> bool {
    let v = value.trim().to_lowercase();
    v.contains("italic") || v.contains("oblique")
}

/// Strip surrounding quotes from a family name.
fn clean_family(name: &str) -> String {
    let n = name.trim();
    let n = n
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| n.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(n);
    n.trim().to_string()
}

/// Decode fetched font bytes: pass sfnt through, decompress WOFF 1.0/2.0.
fn decode_font_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 4 {
        return None;
    }
    match &bytes[0..4] {
        // Bare sfnt: TrueType, 'true', CFF (OTTO), or a collection.
        [0x00, 0x01, 0x00, 0x00]
        | [b't', b'r', b'u', b'e']
        | [b'O', b'T', b'T', b'O']
        | [b't', b't', b'c', b'f'] => Some(bytes.to_vec()),
        [b'w', b'O', b'F', b'F'] => decode_woff(bytes),
        [b'w', b'O', b'F', b'2'] => decode_woff2(bytes),
        _ => None,
    }
}

/// Assemble decoded tables into an sfnt font: 12-byte header, table records,
/// then 4-byte-aligned table data. Returns `None` when the result would be
/// implausibly large (corrupt input).
fn build_sfnt(flavor: u32, tables: &[([u8; 4], Vec<u8>)]) -> Option<Vec<u8>> {
    let num_tables = tables.len();
    let data_start = 12 + num_tables * 16;
    let mut total: u64 = data_start as u64;
    for (_, data) in tables {
        total += (data.len() + 3) as u64 / 4 * 4;
    }
    if total > (2 * MAX_FONT_BYTES) as u64 {
        return None;
    }
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&flavor.to_be_bytes());
    out.extend_from_slice(&(num_tables as u16).to_be_bytes());
    // searchRange / entrySelector / rangeShift: conventional values for the
    // binary-search table directory.
    let entry_selector = (usize::BITS - num_tables.leading_zeros() - 1) as u16;
    let search_range = (1u16 << entry_selector) * 16;
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&((num_tables as u16 * 16) - search_range).to_be_bytes());

    let mut offset = data_start;
    for (tag, data) in tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&0u32.to_be_bytes()); // checksum: validators tolerate zero
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += data.len() + (4 - data.len() % 4) % 4;
    }
    for (_, data) in tables {
        out.extend_from_slice(data);
        let pad = (4 - data.len() % 4) % 4;
        out.extend(std::iter::repeat(0u8).take(pad));
    }
    Some(out)
}

/// Decompress a WOFF 1.0 container back into an sfnt font.
///
/// Layout (per the WOFF spec): a 44-byte header, one 20-byte directory entry
/// per table, then the compressed table payloads at 4-byte-aligned offsets.
/// Rebuilding the sfnt means emitting a new table directory (16-byte records)
/// followed by the table data.
fn decode_woff(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 44 {
        return None;
    }
    let flavor = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let num_tables = u16::from_be_bytes([bytes[12], bytes[13]]) as usize;
    // Real fonts carry a few dozen tables; a larger count means corruption.
    if num_tables == 0 || num_tables > 512 {
        return None;
    }
    let dir_end = 44 + num_tables * 20;
    if bytes.len() < dir_end {
        return None;
    }

    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(num_tables);
    for i in 0..num_tables {
        let e = 44 + i * 20;
        let tag = [bytes[e], bytes[e + 1], bytes[e + 2], bytes[e + 3]];
        let offset =
            u32::from_be_bytes([bytes[e + 4], bytes[e + 5], bytes[e + 6], bytes[e + 7]]) as usize;
        let comp =
            u32::from_be_bytes([bytes[e + 8], bytes[e + 9], bytes[e + 10], bytes[e + 11]]) as usize;
        let orig = u32::from_be_bytes([bytes[e + 12], bytes[e + 13], bytes[e + 14], bytes[e + 15]])
            as usize;
        if offset.checked_add(comp)? > bytes.len() || orig > MAX_FONT_BYTES {
            return None;
        }
        let payload = &bytes[offset..offset + comp];
        let data = if comp == orig {
            payload.to_vec()
        } else {
            let decoder = flate2::read::ZlibDecoder::new(payload);
            // Cap the decompressed size at the declared original length so a
            // corrupt stream cannot balloon memory before the length check.
            let mut out = Vec::with_capacity(orig);
            use std::io::Read as _;
            decoder.take(orig as u64).read_to_end(&mut out).ok()?;
            if out.len() != orig {
                return None;
            }
            out
        };
        tables.push((tag, data));
    }

    build_sfnt(flavor, &tables)
}

/// Known table tags indexed by the WOFF 2.0 directory's 6-bit tag field.
const WOFF2_KNOWN_TAGS: [[u8; 4]; 63] = [
    *b"cmap", *b"head", *b"hhea", *b"hmtx", *b"maxp", *b"name", *b"OS/2", *b"post", *b"cvt ",
    *b"fpgm", *b"glyf", *b"loca", *b"prep", *b"CFF ", *b"VORG", *b"EBDT", *b"EBLC", *b"gasp",
    *b"hdmx", *b"kern", *b"LTSH", *b"PCLT", *b"VDMX", *b"vhea", *b"vmtx", *b"BASE", *b"GDEF",
    *b"GPOS", *b"GSUB", *b"EBSC", *b"JSTF", *b"MATH", *b"CBDT", *b"CBLC", *b"COLR", *b"CPAL",
    *b"SVG ", *b"sbix", *b"acnt", *b"avar", *b"bdat", *b"bloc", *b"bsln", *b"cvar", *b"fdsc",
    *b"feat", *b"fmtx", *b"fvar", *b"gvar", *b"hsty", *b"just", *b"lcar", *b"mort", *b"morx",
    *b"opbd", *b"prop", *b"trak", *b"Zapf", *b"Silf", *b"Glat", *b"Gloc", *b"Feat", *b"Sill",
];

/// Read a WOFF 2.0 UIntBase128 varint.
fn read_base128(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result: u64 = 0;
    for i in 0..5 {
        let b = *bytes.get(*pos)?;
        *pos += 1;
        // The value must be minimally encoded: no continuation on the last
        // permitted byte and no redundant leading zero byte.
        if i == 4 && b & 0x80 != 0 {
            return None;
        }
        if i == 0 && b == 0x80 {
            return None;
        }
        result = (result << 7) | (b & 0x7f) as u64;
        if b & 0x80 == 0 {
            return Some(result);
        }
    }
    None
}

/// Decompress a WOFF 2.0 container back into an sfnt font.
///
/// Layout (per the WOFF 2 spec): a 48-byte header, a variable-length table
/// directory whose entries reference tags by a 6-bit index into a fixed
/// known-tag list, then one brotli stream holding every table's data
/// concatenated in directory order. The glyf/loca transform (TrueType glyph
/// streams split into substreams for compression) is decoded back to standard
/// glyf/loca tables; other tables must be stored untransformed.
fn decode_woff2(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 48 {
        return None;
    }
    let flavor = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let num_tables = u16::from_be_bytes([bytes[12], bytes[13]]) as usize;
    if num_tables == 0 || num_tables > 512 {
        return None;
    }
    let total_compressed =
        u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) as usize;

    struct Entry {
        tag: [u8; 4],
        // Bytes this table contributes to the decompressed stream: the
        // original length for untransformed tables, the transform length for
        // transformed ones (0 for loca, which is rebuilt from glyf).
        stream_length: usize,
        transformed: bool,
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(num_tables);
    let mut pos = 48usize;
    let mut total_uncompressed: u64 = 0;
    for _ in 0..num_tables {
        let flags = *bytes.get(pos)?;
        pos += 1;
        let tag_index = (flags & 0x3f) as usize;
        let tag = if tag_index == 63 {
            let t = bytes.get(pos..pos + 4)?;
            [*t.get(0)?, *t.get(1)?, *t.get(2)?, *t.get(3)?]
        } else {
            WOFF2_KNOWN_TAGS[tag_index]
        };
        if tag_index == 63 {
            pos += 4;
        }
        let orig_length = read_base128(bytes, &mut pos)? as usize;
        if orig_length > MAX_FONT_BYTES {
            return None;
        }
        // Bits 6-7 encode a transform version: for most tables 0 is the null
        // transform, but glyf/loca reserve version 3 for the null transform
        // and use 0-2 for their stream-split encodings.
        let transformed = if &tag == b"glyf" || &tag == b"loca" {
            flags >> 6 != 3
        } else {
            flags >> 6 != 0
        };
        let stream_length = if transformed {
            // The transform's length field must be present; only glyf/loca
            // transforms are understood, and loca's must be empty.
            let tl = read_base128(bytes, &mut pos)? as usize;
            if &tag != b"glyf" && &tag != b"loca" {
                return None;
            }
            if &tag == b"loca" && tl != 0 {
                return None;
            }
            tl
        } else {
            orig_length
        };
        total_uncompressed += stream_length as u64;
        entries.push(Entry {
            tag,
            stream_length,
            transformed,
        });
    }
    if total_uncompressed > (2 * MAX_FONT_BYTES) as u64 {
        return None;
    }
    if pos + total_compressed > bytes.len() {
        return None;
    }

    let mut decompressed = Vec::with_capacity(total_uncompressed as usize);
    use std::io::Read as _;
    let ok = brotli::Decompressor::new(&bytes[pos..pos + total_compressed], 4096)
        .take(total_uncompressed)
        .read_to_end(&mut decompressed)
        .is_ok();
    if !ok {
        return None;
    }
    if decompressed.len() as u64 != total_uncompressed {
        return None;
    }

    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::with_capacity(num_tables);
    let mut cursor = 0usize;
    let mut glyf_entry: Option<usize> = None;
    let mut loca_entry: Option<usize> = None;
    for e in entries.iter() {
        let start = cursor;
        let end = start + e.stream_length;
        if end > decompressed.len() {
            return None;
        }
        cursor = end;
        if e.transformed {
            // The transformed loca contributes no bytes (transformLength 0);
            // it is rebuilt from the reconstructed glyf below.
            if &e.tag == b"glyf" {
                glyf_entry = Some(tables.len());
                tables.push((e.tag, decompressed[start..end].to_vec()));
            } else if &e.tag == b"loca" {
                loca_entry = Some(tables.len());
                tables.push((e.tag, Vec::new()));
            }
        } else {
            tables.push((e.tag, decompressed[start..end].to_vec()));
        }
    }
    if let (Some(g), Some(l)) = (glyf_entry, loca_entry) {
        let (glyf, loca) = reconstruct_glyf_loca(&tables[g].1)?;
        tables[g].1 = glyf;
        tables[l].1 = loca;
    } else if glyf_entry.is_some() || loca_entry.is_some() {
        return None;
    }
    build_sfnt(flavor, &tables)
}

/// A cursor over font data with bounds-checked big-endian readers.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }
    fn u8(&mut self) -> Option<u8> {
        let b = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }
    fn u16(&mut self) -> Option<u16> {
        let b = self.data.get(self.pos..self.pos + 2)?;
        self.pos += 2;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }
    fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }
    fn u32(&mut self) -> Option<u32> {
        let b = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
}

/// Read a WOFF 2.0 255UInt16 variable-length unsigned integer.
fn read_255u16(r: &mut Reader) -> Option<u32> {
    let code = r.u8()?;
    match code {
        253 => Some(r.u16()? as u32),
        254 => Some(r.u8()? as u32 + 506),
        255 => Some(r.u8()? as u32 + 253),
        c => Some(c as u32),
    }
}

/// Decode the WOFF 2.0 glyf transform back into standard glyf and loca
/// tables. The transform splits every glyph record into per-field streams
/// (contour counts, point counts, flags, coordinate triplets, composite
/// components, bounding boxes, instructions) so similar bytes sit together
/// for compression. Reconstruction reassembles standard glyph records and
/// records each glyph's offset to rebuild loca.
fn reconstruct_glyf_loca(transformed_glyf: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let debug = std::env::var("INCOGNIDIUM_WEBFONT_DEBUG").is_ok();
    let _ = &debug;
    let mut r = Reader::new(transformed_glyf);
    let _version = r.u16()?;
    let option_flags = r.u16()?;
    let num_glyphs = r.u16()? as usize;
    let index_format = r.u16()?;
    if index_format > 1 || num_glyphs == 0 {
        return None;
    }
    let stream_sizes: Vec<usize> = (0..7)
        .map(|_| r.u32().map(|v| v as usize))
        .collect::<Option<Vec<_>>>()?;
    let [n_contour_len, n_points_len, flag_len, glyph_len, composite_len, bbox_len, instruction_len] =
        stream_sizes[..]
    else {
        return None;
    };
    if n_contour_len != num_glyphs * 2 {
        return None;
    }
    let mut n_contours = Reader::new(r.take(n_contour_len)?);
    let mut n_points = Reader::new(r.take(n_points_len)?);
    let flags = r.take(flag_len)?;
    let mut glyph = Reader::new(r.take(glyph_len)?);
    let mut composite = Reader::new(r.take(composite_len)?);
    let bbox_stream = r.take(bbox_len)?;
    let mut instructions = Reader::new(r.take(instruction_len)?);
    let has_overlap_bitmap = option_flags & 1 != 0;
    let overlap_bitmap = if has_overlap_bitmap {
        r.take((num_glyphs + 7) / 8)?
    } else {
        &[]
    };
    // The bbox bitmap precedes the bbox values inside the bbox stream.
    let bbox_bitmap_len = ((num_glyphs + 31) / 32) * 4;
    if bbox_bitmap_len > bbox_stream.len() {
        return None;
    }
    let bbox_bitmap = &bbox_stream[..bbox_bitmap_len];
    let mut bbox = Reader::new(&bbox_stream[bbox_bitmap_len..]);
    if debug {
        eprintln!(
            "[glyf] glyphs={num_glyphs} idx={index_format} streams={} pos={} len={}",
            stream_sizes
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
                .join(","),
            r.pos,
            transformed_glyf.len()
        );
    }
    if r.pos != transformed_glyf.len() {
        return None;
    }

    let mut glyf_out: Vec<u8> = Vec::with_capacity(transformed_glyf.len() * 2);
    let mut loca_offsets: Vec<u32> = Vec::with_capacity(num_glyphs + 1);
    let mut flag_pos = 0usize;
    for gid in 0..num_glyphs {
        // TrueType glyph records are 2-byte aligned so short-format loca
        // offsets stay representable; pad any odd-length record.
        if glyf_out.len() % 2 != 0 {
            glyf_out.push(0);
        }
        loca_offsets.push(glyf_out.len() as u32);
        let n_contour = n_contours.i16()?;
        if n_contour == 0 {
            continue;
        }
        let overlap_bit = if has_overlap_bitmap {
            overlap_bitmap.get(gid / 8).copied().unwrap_or(0) & (0x80 >> (gid % 8)) != 0
        } else {
            false
        };
        let have_bbox = bbox_bitmap.get(gid / 8).copied().unwrap_or(0) & (0x80 >> (gid % 8)) != 0;
        if n_contour < 0 {
            // Composite glyph: components use the standard TrueType
            // encoding, so their bytes are copied through unchanged.
            glyf_out.extend_from_slice(&n_contour.to_be_bytes());
            // The composite's bounding box lives only in the transform's
            // bbox stream; the standard record still opens with it, so
            // re-insert the four values into the glyph record.
            if !have_bbox {
                return None;
            }
            for _ in 0..4 {
                glyf_out.extend_from_slice(&bbox.i16()?.to_be_bytes());
            }
            let mut have_instructions = false;
            loop {
                let start = composite.pos;
                let flags_word = composite.u16()?;
                composite.u16()?; // glyph index
                if flags_word & 0x0001 != 0 {
                    composite.take(4)?; // arg1, arg2 as 16-bit values
                } else {
                    composite.take(2)?; // arg1, arg2 as bytes
                }
                if flags_word & 0x0008 != 0 {
                    composite.take(2)?; // F2Dot14 scale
                } else if flags_word & 0x0040 != 0 {
                    composite.take(4)?; // x and y scales
                } else if flags_word & 0x0080 != 0 {
                    composite.take(8)?; // 2x2 transform
                }
                glyf_out.extend_from_slice(&composite.data[start..composite.pos]);
                if flags_word & 0x0100 != 0 {
                    have_instructions = true;
                }
                if flags_word & 0x0020 == 0 {
                    break;
                }
            }
            if have_instructions {
                let ilen = read_255u16(&mut glyph)? as usize;
                let bytes = instructions.take(ilen)?;
                glyf_out.extend_from_slice(&(ilen as u16).to_be_bytes());
                glyf_out.extend_from_slice(bytes);
            }
        } else {
            // Simple glyph: rebuild the outline from the substreams.
            let mut end_pts: Vec<u32> = Vec::with_capacity(n_contour as usize);
            let mut end: i32 = -1;
            for _ in 0..n_contour {
                let n = read_255u16(&mut n_points)?;
                end += n as i32;
                end_pts.push(end as u32);
            }
            let n_points_total = end + 1;
            if n_points_total < 0 || flag_pos + n_points_total as usize > flags.len() {
                return None;
            }
            let point_flags = &flags[flag_pos..flag_pos + n_points_total as usize];
            flag_pos += n_points_total as usize;
            // Decode the triplet-encoded point deltas. Coordinates are
            // deltas from the previous point; the sign bit for x is flag
            // bit 0 and for y flag bit 1.
            let mut coords: Vec<(i32, i32, bool)> = Vec::with_capacity(n_points_total as usize);
            let mut x: i32 = 0;
            let mut y: i32 = 0;
            for &pf in point_flags {
                let on_curve = pf >> 7 == 0;
                let pf = pf & 0x7f;
                let signed = |base: i32, negative: bool| -> Option<i32> {
                    if base < 0 || base >= 65536 {
                        return None;
                    }
                    Some(if negative { -base } else { base })
                };
                let (dx, dy) = match pf {
                    0..=9 => (
                        0,
                        signed(((pf as i32 & 14) << 7) + glyph.u8()? as i32, pf & 1 == 0)?,
                    ),
                    10..=19 => (
                        signed(
                            (((pf as i32 - 10) & 14) << 7) + glyph.u8()? as i32,
                            pf & 1 == 0,
                        )?,
                        0,
                    ),
                    20..=83 => {
                        let b0 = (pf - 20) as i32;
                        let b1 = glyph.u8()? as i32;
                        (
                            signed(1 + (b0 & 0x30) + (b1 >> 4), pf & 1 == 0)?,
                            signed(1 + ((b0 & 0x0C) << 2) + (b1 & 0x0F), pf & 2 == 0)?,
                        )
                    }
                    84..=119 => {
                        let b0 = (pf - 84) as i32;
                        let b1 = glyph.u8()? as i32;
                        let b2 = glyph.u8()? as i32;
                        (
                            signed(1 + ((b0 / 12) << 8) + b1, pf & 1 == 0)?,
                            signed(1 + (((b0 % 12) >> 2) << 8) + b2, pf & 2 == 0)?,
                        )
                    }
                    120..=123 => {
                        let b1 = glyph.u8()? as i32;
                        let b2 = glyph.u8()? as i32;
                        let b3 = glyph.u8()? as i32;
                        (
                            signed((b1 << 4) + (b2 >> 4), pf & 1 == 0)?,
                            signed(((b2 & 0x0F) << 8) + b3, pf & 2 == 0)?,
                        )
                    }
                    _ => {
                        let b1 = glyph.u8()? as i32;
                        let b2 = glyph.u8()? as i32;
                        let b3 = glyph.u8()? as i32;
                        let b4 = glyph.u8()? as i32;
                        (
                            signed((b1 << 8) + b2, pf & 1 == 0)?,
                            signed((b3 << 8) + b4, pf & 2 == 0)?,
                        )
                    }
                };
                x += dx;
                y += dy;
                coords.push((x, y, on_curve));
            }
            let ilen = read_255u16(&mut glyph)? as usize;
            let instr = instructions.take(ilen)?;
            // Bounding box: explicit values when flagged, otherwise derived
            // from the outline points.
            let (x_min, y_min, x_max, y_max) = if have_bbox {
                (bbox.i16()?, bbox.i16()?, bbox.i16()?, bbox.i16()?)
            } else if let Some(&(x0, y0, _)) = coords.first() {
                let (mut lo_x, mut hi_x, mut lo_y, mut hi_y) = (x0, x0, y0, y0);
                for &(cx, cy, _) in &coords {
                    lo_x = lo_x.min(cx);
                    hi_x = hi_x.max(cx);
                    lo_y = lo_y.min(cy);
                    hi_y = hi_y.max(cy);
                }
                (lo_x as i16, lo_y as i16, hi_x as i16, hi_y as i16)
            } else {
                (0, 0, 0, 0)
            };
            // Assemble the standard simple-glyph record: contour count,
            // bounding box, contour endpoints, instructions, then per-point
            // flags followed by all x deltas and all y deltas.
            glyf_out.extend_from_slice(&n_contour.to_be_bytes());
            for v in [x_min, y_min, x_max, y_max] {
                glyf_out.extend_from_slice(&v.to_be_bytes());
            }
            for &e in &end_pts {
                glyf_out.extend_from_slice(&(e as u16).to_be_bytes());
            }
            glyf_out.extend_from_slice(&(ilen as u16).to_be_bytes());
            glyf_out.extend_from_slice(instr);
            for (i, &(_, _, on_curve)) in coords.iter().enumerate() {
                let mut flag = if on_curve { 0x01u8 } else { 0x00u8 };
                if i == 0 && overlap_bit {
                    flag |= 0x40;
                }
                glyf_out.push(flag);
            }
            // The record stores deltas from the previous point; the first
            // point's delta is relative to the origin.
            let mut px = 0i32;
            let mut py = 0i32;
            for &(cx, _cy, _) in &coords {
                glyf_out.extend_from_slice(&((cx - px) as i16).to_be_bytes());
                px = cx;
            }
            for &(_cx, cy, _) in &coords {
                glyf_out.extend_from_slice(&((cy - py) as i16).to_be_bytes());
                py = cy;
            }
        }
    }
    loca_offsets.push(glyf_out.len() as u32);
    let mut loca_out: Vec<u8> = Vec::with_capacity(loca_offsets.len() * 2);
    if index_format == 0 {
        for o in &loca_offsets {
            if o % 2 != 0 || *o > 0x1fffe {
                return None;
            }
            loca_out.extend_from_slice(&((o / 2) as u16).to_be_bytes());
        }
    } else {
        for o in &loca_offsets {
            loca_out.extend_from_slice(&o.to_be_bytes());
        }
    }
    Some((glyf_out, loca_out))
}

/// Decode a `data:` URI payload into raw bytes.
fn decode_data_uri(uri: &str) -> Option<Vec<u8>> {
    let rest = uri.strip_prefix("data:")?;
    let payload = rest.split(',').nth(1)?;
    let cleaned: Vec<u8> = payload
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    decode_base64(&cleaned)
}

/// Minimal base64 decoder (standard alphabet, with padding).
fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    fn value(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a') as u32 + 26),
            b'0'..=b'9' => Some((b - b'0') as u32 + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    for &b in input {
        if b == b'=' {
            break;
        }
        let v = value(b)?;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
    }
    Some(out)
}

/// Download and register every usable face declared by the stylesheet.
///
/// `resolve_and_fetch` receives the document base URL and the raw `src`
/// attribute of a face and returns the raw font file bytes (empty when the
/// resource is unavailable); the caller supplies its own resolver/fetcher so
/// this crate needs no networking. Faces whose format is not decodable
/// (e.g. WOFF 2 only) are skipped silently.
pub fn load_from_stylesheet(
    sheet: &Stylesheet,
    base_url: &str,
    resolve_and_fetch: &dyn Fn(&str, &str) -> Vec<u8>,
) {
    clear();
    let debug = std::env::var("INCOGNIDIUM_WEBFONT_DEBUG").is_ok();
    let mut registered = 0usize;
    for face in &sheet.font_faces {
        if registered >= MAX_FACES {
            break;
        }
        let Some(family) = face.font_family.as_deref().map(clean_family) else {
            continue;
        };
        if family.is_empty() {
            continue;
        }
        let Some(src) = face.src.as_deref() else {
            continue;
        };
        // SVG-in-OpenType fonts cannot be decoded; skip the face rather than
        // registering a broken font. The parser keeps the last src candidate,
        // which in the common `url(x.woff2) format("woff2"), url(y.woff)
        // format("woff")` list is the decodable fallback.
        if let Some(format) = face.format.as_deref() {
            if format.trim().to_lowercase().contains("svg") {
                continue;
            }
        }
        let bytes: Option<Vec<u8>> = if src.starts_with("data:") {
            decode_data_uri(src)
        } else {
            let data = resolve_and_fetch(base_url, src);
            if data.is_empty() || data.len() > MAX_FONT_BYTES {
                None
            } else {
                Some(data)
            }
        };
        let Some(raw) = bytes else {
            if debug {
                eprintln!(
                    "[webfont] {} <- {} fetch failed or too large",
                    family,
                    &src.chars().take(80).collect::<String>()
                );
            }
            continue;
        };
        let Some(decoded) = decode_font_bytes(&raw) else {
            if debug {
                eprintln!(
                    "[webfont] {} <- {} undecodable ({})",
                    family,
                    &src.chars().take(80).collect::<String>(),
                    raw.len()
                );
            }
            continue;
        };
        if debug {
            eprintln!(
                "[webfont] registered {} weight={} italic={} ({} bytes)",
                family,
                face.font_weight.as_deref().unwrap_or("normal"),
                face.font_style.as_deref().unwrap_or("normal"),
                decoded.len()
            );
        }
        register_face(
            &family,
            parse_weight(face.font_weight.as_deref().unwrap_or("normal")),
            parse_italic(face.font_style.as_deref().unwrap_or("normal")),
            decoded,
        );
        registered += 1;
    }
}

/// Convenience wrapper used by tests and tools that have no real fetcher:
/// loads faces from `data:` URIs only and ignores network sources.
pub fn load_from_css_text(css_text: &str) -> usize {
    let sheet = crate::parse_css(css_text);
    load_from_stylesheet(&sheet, "", &|_base, _src| Vec::new());
    registry().lock().map(|r| r.len()).unwrap_or(0)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Decode a real WOFF file from disk when one is available for manual
    /// debugging (run with: cargo test -p incognidium-css decode_woff_file -- --ignored --nocapture)
    #[test]
    #[ignore]
    fn decode_woff_file() {
        let path = std::env::var("WOFF_TEST_FILE").unwrap_or_default();
        if path.is_empty() {
            return;
        }
        let bytes = std::fs::read(&path).unwrap();
        let decoded = decode_font_bytes(&bytes);
        println!("decoded: {:?}", decoded.as_ref().map(|d| d.len()));
        if let Some(out) = std::env::var("WOFF_OUT").ok().filter(|_| decoded.is_some()) {
            std::fs::write(out, decoded.clone().unwrap()).unwrap();
        }
        assert!(decoded.is_some());
    }
}
