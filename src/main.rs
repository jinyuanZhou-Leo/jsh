mod builtin;
use std::io::{self, Write};

use crate::builtin::{echo, exit};

fn main() {
    loop {
        print!("$ ");
        io::stdout().flush().unwrap();

        let mut command = String::new();
        io::stdin().read_line(&mut command).unwrap();
        let command: Vec<&str> = command.trim().split(' ').collect();

        match command[0] {
            "exit" => {
                exit();
            }
            "echo" => {
                echo(command[1..].join(" "));
            }
            unknown_command => {
                print!("{}: command not found\n", unknown_command);
                io::stdout().flush().unwrap();
            }
        }
    }
}
