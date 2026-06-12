# Multiple Rust Programs in One Repo

For small demos and GTK experiments, use Cargo’s `examples/` directory.

## Structure

```text
myproject/
├── Cargo.toml
├── src/
│   └── main.rs
└── examples/
    ├── hello.rs
    ├── buttons.rs
    └── sqlite_demo.rs
```

## Run an example

```bash
cargo run --example hello
```

## Build all examples

```bash
cargo build --examples
cargo check --examples
```

## Shared code

Put reusable code in `src/lib.rs`:

```text
src/
├── lib.rs
└── ui/
    └── helpers.rs
```

Then use it from examples:

```rust
use myproject::ui::helpers::*;
```

## Why use `examples/`

- ideal for experiments/tutorials
- each file is an independent program
- shares dependencies automatically
- simple Cargo workflow
- great for GTK learning projects
