mod builtin;
use std::io::{self, Write};

use crate::builtin::exit;

fn main() {
    loop {
        print!("$ ");
        io::stdout().flush().unwrap();

        let mut command = String::new();
        io::stdin().read_line(&mut command).unwrap();
        let command = command.trim();

        match command {
            "exit" => {
                exit();
            }
            unknown_command => {
                print!("{}: command not found\n", unknown_command);
                io::stdout().flush().unwrap();
            }
        }
        
    }
}
