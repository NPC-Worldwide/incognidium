fn main() {
    let css = r#"background: url('data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect fill="red" width="10" height="10"/></svg>') repeat;"#;
    
    // Find url(
    if let Some(url_start) = css.find("url(") {
        let url_idx = url_start + 4;
        let remaining = &css[url_idx..];
        
        // Find closing paren
        let mut depth = 1;
        let mut close_idx = 0;
        for (i, c) in remaining.chars().enumerate() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        close_idx = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        
        let url_content = &remaining[..close_idx];
        println!("URL content: {}", url_content);
        
        // Remove quotes
        let url_content = url_content.trim();
        let url_content = url_content.strip_prefix('"').unwrap_or(url_content);
        let url_content = url_content.strip_prefix('\'').unwrap_or(url_content);
        let url_content = url_content.strip_suffix('"').unwrap_or(url_content);
        let url_content = url_content.strip_suffix('\'').unwrap_or(url_content);
        
        println!("Cleaned URL: {}", url_content);
        println!("Starts with data?: {}", url_content.starts_with("data:"));
    }
}
