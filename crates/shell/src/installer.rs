use std::io::{self, BufRead, Write};
use std::time::Duration;
use std::thread;

use crate::color::{color, Color};

pub fn print_missing(tool: &str, package: &str) {
    println!();
    println!("  ┌─────────────────────────────────────┐");
    println!("  │       Tool Installation              ");
    println!("  │                                     ");
    println_field("Tool", tool);
    println_field("Package", package);
    println!("  └─────────────────────────────────────┘");
    println!();
}

pub fn print_success() {
    println!();
    println!("  {} {}", color(Color::Green, "✓"), color(Color::Green, "Installed successfully!"));
}

pub fn print_failure(_err: &str) {
    println!();
    println!("  {} {}", color(Color::Red, "✗"), color(Color::Red, "Installation failed"));
}

fn println_field(label: &str, value: &str) {
    println!("  │  {}{}: {}", color(Color::Cyan, label), color(Color::Reset, ""), color(Color::Reset, value));
}

pub fn prompt_install(tool: &str, package: &str) -> bool {
    print_missing(tool, package);
    
    print!("  {} [y/N]: ", color(Color::Reset, "Install now"));
    io::stdout().flush().ok();
    
    let mut input = String::new();
    if io::stdin().lock().read_line(&mut input).is_err() {
        return false;
    }
    
    let input = input.trim().to_lowercase();
    input == "y" || input == "yes"
}

pub fn show_spinner(duration_ms: u64) {
    let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let iterations = (duration_ms / 80) as usize;
    
    for i in 0..iterations {
        print!("\r  {} ", color(Color::Yellow, spinner_frames[i % spinner_frames.len()]));
        io::stdout().flush().ok();
        thread::sleep(Duration::from_millis(80));
    }
    print!("\r      \r");
    io::stdout().flush().ok();
}