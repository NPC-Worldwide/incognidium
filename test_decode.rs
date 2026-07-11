use urlencoding::decode;

fn main() {
    // This is the raw SVG data - not URL encoded
    let data = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect fill=\"red\" width=\"10\" height=\"10\"/></svg>";
    
    match decode(data) {
        Ok(decoded) => {
            println!("Decoded OK");
            println!("Decoded: {}", decoded);
            println!("Decoded bytes: {:?}", decoded.as_bytes());
        }
        Err(e) => {
            println!("Decode error: {:?}", e);
        }
    }
}
