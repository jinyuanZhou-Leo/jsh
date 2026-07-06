use std::{collections::HashMap, error::Error};

use crate::{
    lexer::{Token, WordPart},
    shell::Shell,
};

pub struct Expander;

impl Expander {
    pub fn new() -> Self {
        Self {}
    }

    pub fn expand(&self, shell: &Shell, input: Vec<Token>) -> Result<Vec<String>, Box<dyn Error>> {
        let mut output: Vec<String> = Vec::new();
        for token in input {
            match token {
                Token::Word(word_parts) => {
                    let mut word_buffer: String = String::new();
                    for word_part in word_parts {
                        match word_part {
                            WordPart::Normal(content) => {
                                let content = Self::expand_tilde(shell.env_vars(), content)?;
                                word_buffer.push_str(&content);
                            }
                            WordPart::SingleQuoted(literal) => {
                                word_buffer.push_str(&literal);
                            }
                            WordPart::DoubleQuoted(content) => {
                                // TODO: treated as literal FOR NOW
                                word_buffer.push_str(&content);
                            }
                        }
                    }

                    output.push(word_buffer);
                }
                _ => {}
            }
        }

        Ok(output)
    }

    fn expand_tilde(
        env_vars: &HashMap<String, String>,
        line: String,
    ) -> Result<String, Box<dyn Error>> {
        let Some(home_dir) = env_vars.get("HOME") else {
            return Err("Could not find env var 'HOME'".into());
        };

        Ok(line.replace('~', home_dir))
    }
}
