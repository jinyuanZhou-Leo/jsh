#[allow(unused_imports)]
use std::io::{self, Write};

fn main() {
    
    print!("$ ");
    io::stdout().flush().unwrap();

    let mut command = String::new();
    io::stdin().read_line(&mut command).unwrap();
    command.remove(command.len()-1);
    print!("{}: command not found\n", command);
    io::stdout().flush().unwrap();
}
