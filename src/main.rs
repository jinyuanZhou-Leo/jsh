#[allow(unused_imports)]
use std::io::{self, Write};

fn main() {
    
    print!("$ ");
    io::stdout().flush().unwrap();

    let mut command = String::new();
    io::stdin().read_line(&mut command).unwrap();
    print!("{command}: command not found");
    io::stdout().flush().unwrap();
}
