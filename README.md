# jsh

一个使用 Rust 编写的交互式 shell 学习项目，基于 [CodeCrafters Build Your Own Shell](https://app.codecrafters.io/courses/shell/overview) 实现。

[中文](README.md) | [English](README.en.md)

> 这是一个用于学习 shell 工作原理的实现，目前只覆盖 POSIX shell 的一部分语法，并不宣称完全兼容 POSIX。

## 功能

- 交互式 REPL，使用 `$ ` 作为提示符
- 内建命令：`cd`、`echo`、`exit`、`pwd`、`type`
- 根据 `PATH` 查找并执行外部程序
- 单引号、双引号和反斜杠转义
- 波浪号展开，例如 `~/notes`
- 输入和输出重定向：`<`、`>`、`>>`
- 支持文件描述符 `0`、`1`、`2` 的常用重定向形式，例如 `2>error.log`
- `&&` 短路执行
- 将内建命令和外部命令统一纳入重定向及错误状态码处理

## 项目结构

```text
输入
  │
  ▼
Lexer ──> Token
  │
  ▼
Parser ──> AST
  │
  ▼
Expander ──> 展开的命令
  │
  ▼
Executor ──> 内建命令 / 外部程序
```

核心源码位于 `src/`：

| 文件 | 职责 |
| --- | --- |
| `main.rs` | REPL 和错误边界 |
| `lexer.rs` | 将输入文本切分为 token |
| `parser.rs` | 将 token 解析为命令 AST |
| `expander.rs` | 处理单词和路径展开 |
| `executor.rs` | 处理重定向并执行命令 |
| `shell.rs` | 保存当前目录、环境变量和命令解析状态 |
| `builtin/` | 内建命令实现 |
| `external.rs` | 根据 `PATH` 查找可执行文件 |

## 环境要求

- Rust 1.96 或更高版本
- Unix-like 系统（项目使用 Unix 进程和文件描述符语义）

## 快速开始

克隆仓库并进入项目目录：

```bash
git clone <your-repository-url>
cd codecrafters-shell-rust
```

构建并启动：

```bash
cargo run
```

也可以使用 CodeCrafters 提供的本地启动脚本：

```bash
./your_program.sh
```

启动后可以尝试：

```console
$ pwd
$ echo "hello, shell"
$ type cd
$ printf 'hello\n' > output.txt
$ cat output.txt
$ false && echo "不会执行"
$ exit
```

## 开发与验证

```bash
cargo check
cargo test
cargo fmt --check
```

## 当前边界

以下能力目前尚未实现或尚未完整实现：

- 管道 `|`
- 后台执行 `&`
- Here-document `<<`
- 环境变量展开，例如 `$HOME`
- 通配符展开（globbing）
- 命令替换、分号分隔和更复杂的 shell 控制结构

## 许可证

当前仓库未声明许可证。如需公开分发，请先补充合适的许可证文件。
