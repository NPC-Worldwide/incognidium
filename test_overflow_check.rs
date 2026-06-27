use incognidium_style::{Overflow, ComputedStyle};

fn main() {
    let style = ComputedStyle::default();
    println!("Default overflow: {:?}", style.overflow);
}
