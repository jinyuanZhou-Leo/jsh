# jsh

一个使用 Rust 编写的交互式 shell 学习项目，参考 [CodeCrafters Build Your Own Shell](https://app.codecrafters.io/courses/shell/overview) 中的步骤实现。

[中文](README.md) | [English](README.en.md)

## 功能

- 交互式 REPL，使用 `$ ` 作为提示符
- 将命令历史持久化到 `~/.jsh_history`（最多 1000 条，忽略前导空格和重复项）；可用 `HISTFILE` 指定路径，空值则关闭持久化
- 内建命令：`cd`、`echo`、`exit`、`pwd`、`type`、`jobs`、`fg`、`bg`
- 根据 `PATH` 查找并执行外部程序；含 `/` 的命令名（如 `./script`、`/bin/ls`）直接解析，相对路径和相对 `PATH` 目录相对 Shell 逻辑当前目录，而不是进程当前目录
- 单引号、双引号和反斜杠转义
- 波浪号展开，例如 `~/notes`
- 输入和输出重定向：`<`、`>`、`>>`
- 支持文件描述符 `0`、`1`、`2` 的常用重定向形式，例如 `2>error.log`
- `&&` 和 `||` 短路执行，管道的解析优先级高于条件连接符
- 使用 `|` 连接前台管道，内建命令和外部命令均可作为管道阶段
- 管道启动全部阶段后统一等待，并返回最后一个阶段的状态码
- 支持使用 `&` 启动后台作业，并使用 `jobs`、`fg`、`bg` 管理作业
- 支持作业停止、继续、前后台切换以及进程组和控制终端管理
- 命令自身的重定向会覆盖所在管道阶段的默认输入或输出
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
Executor ──> 条件执行 / 管道 / 单条命令
                         │
                         └──> 内建命令 / 外部程序
```

核心源码位于 `src/`：

| 文件 | 职责 |
| --- | --- |
| `main.rs` | REPL 和错误边界 |
| `lexer.rs` | 将输入文本切分为 token |
| `parser.rs` | 将 token 解析为命令 AST |
| `expander.rs` | 处理单词和路径展开 |
| `executor.rs` | 处理条件执行、管道、重定向和命令分派 |
| `executor/tests.rs` | Executor 单元测试 |
| `shell.rs` | 保存当前目录、环境变量和命令解析状态 |
| `builtin/` | 内建命令实现 |
| `external.rs` | 按逻辑当前目录解析带路径的命令，并在 `PATH` 中查找可执行文件 |
| `tests/` | 真实程序进程的集成测试 |

## 环境要求

- Rust 1.88 或更高版本
- Unix-like 系统（项目使用 Unix 进程和文件描述符语义）

## 快速开始

克隆仓库并进入项目目录：

```bash
git clone https://github.com/jinyuanZhou-Leo/jsh.git
cd jsh
```

构建并启动：

```bash
cargo run
```

启动后可以尝试：

```console
$ pwd
$ echo "hello, shell"
$ type cd
$ cd /tmp
$ ./script
$ printf 'hello\n' > output.txt
$ cat output.txt
$ false && echo "不会执行"
$ false || echo "上一条命令失败"
$ echo "hello from builtin" | cat
$ printf 'pipeline output\n' | cat > pipeline.txt
$ sleep 10 &
$ jobs
$ fg %1
$ exit
```

## 开发与验证

```bash
cargo check
cargo test --all-targets
cargo fmt --check
```

## 当前边界

以下能力目前尚未实现或尚未完整实现：

- `pipefail`
- Here-document `<<`
- 环境变量展开，例如 `$HOME`
- 通配符展开（globbing）
- 命令替换和更复杂的 shell 控制结构

## 许可证

当前仓库未声明许可证。如需公开分发，请先补充合适的许可证文件。
