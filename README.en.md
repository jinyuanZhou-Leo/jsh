# jsh

An interactive shell learning project written in Rust, implemented through [CodeCrafters' Build Your Own Shell](https://app.codecrafters.io/courses/shell/overview).

[中文](README.md) | [English](README.en.md)

> This project implements a learning-oriented subset of POSIX shell syntax. It does not claim full POSIX compatibility.

## Features

- Interactive REPL with a `$ ` prompt
- Built-in commands: `cd`, `echo`, `exit`, `pwd`, and `type`
- External command lookup through `PATH`
- Single quotes, double quotes, and backslash escapes
- Tilde expansion, such as `~/notes`
- Input and output redirection: `<`, `>`, and `>>`
- Common redirection forms for file descriptors `0`, `1`, and `2`, such as `2>error.log`
- Short-circuit execution with `&&`
- Consistent redirection and exit-status handling for built-ins and external commands

## Architecture

```text
Input
  │
  ▼
Lexer ──> Tokens
  │
  ▼
Parser ──> AST
  │
  ▼
Expander ──> Expanded command
  │
  ▼
Executor ──> Built-in / external command
```

The main implementation is under `src/`:

| File | Responsibility |
| --- | --- |
| `main.rs` | REPL and error boundaries |
| `lexer.rs` | Converts source text into tokens |
| `parser.rs` | Converts tokens into a command AST |
| `expander.rs` | Expands words and paths |
| `executor.rs` | Prepares redirections and runs commands |
| `shell.rs` | Stores the current directory, environment, and command state |
| `builtin/` | Built-in command implementations |
| `external.rs` | Finds executables through `PATH` |

## Requirements

- Rust 1.96 or newer
- A Unix-like operating system (the project relies on Unix process and file-descriptor semantics)

## Quick start

Clone the repository and enter its directory:

```bash
git clone <your-repository-url>
cd codecrafters-shell-rust
```

Build and start the shell:

```bash
cargo run
```

You can also use the local runner supplied by CodeCrafters:

```bash
./your_program.sh
```

Once started, try:

```console
$ pwd
$ echo "hello, shell"
$ type cd
$ printf 'hello\n' > output.txt
$ cat output.txt
$ false && echo "this will not run"
$ exit
```

## Development and verification

```bash
cargo check
cargo test
cargo fmt --check
```

## Current limitations

The following features are not implemented or are not fully implemented yet:

- Pipelines: `|`
- Background execution: `&`
- Here-documents: `<<`
- Environment-variable expansion, such as `$HOME`
- Filename expansion (globbing)
- Command substitution, semicolon-separated commands, and more complex shell control structures

## License

The repository currently does not declare a license. Add an appropriate license file before distributing it publicly.
