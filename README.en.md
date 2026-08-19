# jsh

An interactive shell learning project written in Rust, implemented through [CodeCrafters' Build Your Own Shell](https://app.codecrafters.io/courses/shell/overview).

[中文](README.md) | [English](README.en.md)

## Features

- Interactive REPL with a `$ ` prompt
- Persistent command history in `~/.jsh_history` (up to 1000 entries; leading-space and duplicate lines are ignored); set `HISTFILE` to choose another path, or an empty value to disable persistence
- Built-in commands: `cd`, `echo`, `exit`, `pwd`, and `type`
- External command lookup through `PATH`; names containing `/` (such as `./script` or `/bin/ls`) are resolved directly, and relative paths plus relative `PATH` entries use the shell's logical current directory rather than the process cwd
- Single quotes, double quotes, and backslash escapes
- Tilde expansion, such as `~/notes`
- Input and output redirection: `<`, `>`, and `>>`
- Common redirection forms for file descriptors `0`, `1`, and `2`, such as `2>error.log`
- Short-circuit execution with `&&` and `||`, with pipelines binding more tightly than conditionals
- Foreground pipelines connected with `|`, with both built-ins and external commands supported as stages
- All pipeline stages are started before waiting, and the final stage determines the pipeline status
- Command-local redirections override the default input or output of their pipeline stage
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
Executor ──> Conditional / pipeline / simple command
                                  │
                                  └──> Built-in / external command
```

The main implementation is under `src/`:

| File | Responsibility |
| --- | --- |
| `main.rs` | REPL and error boundaries |
| `lexer.rs` | Converts source text into tokens |
| `parser.rs` | Converts tokens into a command AST |
| `expander.rs` | Expands words and paths |
| `executor.rs` | Handles conditionals, pipelines, redirections, and command dispatch |
| `executor/tests.rs` | Executor unit tests |
| `shell.rs` | Stores the current directory, environment, and command state |
| `builtin/` | Built-in command implementations |
| `external.rs` | Resolves path-qualified commands against the logical cwd and finds executables through `PATH` |
| `tests/` | Integration tests that run the real shell process |

## Requirements

- Rust 1.96 or newer
- A Unix-like operating system (the project relies on Unix process and file-descriptor semantics)

## Quick start

Clone the repository and enter its directory:

```bash
git clone https://github.com/jinyuanZhou-Leo/jsh.git
cd jsh
```

Build and start the shell:

```bash
cargo run
```

Once started, try:

```console
$ pwd
$ echo "hello, shell"
$ type cd
$ cd /tmp
$ ./script
$ printf 'hello\n' > output.txt
$ cat output.txt
$ false && echo "this will not run"
$ false || echo "the previous command failed"
$ echo "hello from builtin" | cat
$ printf 'pipeline output\n' | cat > pipeline.txt
$ exit
```

## Development and verification

```bash
cargo check
cargo test --all-targets
cargo fmt --check
```

## Current limitations

The following features are not implemented or are not fully implemented yet:

- Background execution: `&`
- Full job control, process groups, and `pipefail`
- Here-documents: `<<`
- Environment-variable expansion, such as `$HOME`
- Filename expansion (globbing)
- Command substitution, semicolon-separated commands, and more complex shell control structures

## License

The repository currently does not declare a license. Add an appropriate license file before distributing it publicly.
