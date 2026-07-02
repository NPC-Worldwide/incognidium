fn main() {
    // This is what the CSS contains after parsing
    let data_uri = "data:image/svg+xml,<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"20\" height=\"20\"><rect fill=\"red\" width=\"10\" height=\"10\"/></svg>";
    
    println!("Full URI: {}", data_uri);
    
    let after_data = &data_uri[5..];
    let comma_pos = after_data.find(',').unwrap();
    let meta = &after_data[..comma_pos];
    let data_part = &after_data[comma_pos + 1..];
    
    println!("Meta: {}", meta);
    println!("Data part: {}", data_part);
    println!("Data part bytes: {:?}", data_part.as_bytes());
}
