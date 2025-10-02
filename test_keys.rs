use ratatui::crossterm::event::{self, Event, KeyCode, KeyModifiers};
use std::io;

fn main() -> io::Result<()> {
    println!("Press Ctrl+D or Ctrl+U (press 'q' to quit):");
    
    loop {
        if event::poll(std::time::Duration::from_millis(500))? {
            if let Event::Key(key) = event::read()? {
                if key.code == KeyCode::Char('q') {
                    break;
                }
                println!("Key: {:?}, Modifiers: {:?}", key.code, key.modifiers);
                
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    if let KeyCode::Char(c) = key.code {
                        println!("  -> Detected Ctrl+{}", c);
                    }
                }
            }
        }
    }
    
    Ok(())
}
