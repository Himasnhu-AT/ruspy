# Ruspy

A simple interpreter written in Rust that supports basic variable declarations, printing output, and mathematical calculations following BODMAS (Brackets, Orders, Division, Multiplication, Addition, Subtraction).

## Features

- Variable declaration and assignment with optional type annotations
- Mathematical operations with proper operator precedence (BODMAS)
- Print statements for output
- String and numeric literal support

## Usage

Run the interpreter on a source file:

```bash
cargo run -- path/to/file.ruspy
```

With debug mode to print AST and execution flow:

```bash
cargo run -- -d path/to/file.ruspy
```

## Example Code

```ruspy
// Variable declarations
x: int64 = 10;
y: int64 = 5;

// Mathematical operations following BODMAS
result: int64 = (x + y) * 2 / (5 - 2);
print result;  // Should output 10

// Complex expression
complex: int64 = x * (y + 3) - 2 ^ 2;
print complex;  // Should calculate following BODMAS

// String output
message: str = "Hello, Ruspy!";
print message;
```

## Implementation Details

The interpreter follows a classic compiler pipeline:
1. Lexer: Tokenizes the source code
2. Parser: Builds an Abstract Syntax Tree (AST) from tokens
3. Interpreter: Evaluates the AST and executes the program

## Getting Started

To get started with Ruspy, clone the repository and follow the instructions below.

### Prerequisites

- Rust 2021 edition

### Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/Himasnhu-at/ruspy.git
   ```
2. Navigate to the project directory:
   ```bash
   cd ruspy
   ```
3. Build the project:
   ```bash
   cargo build
   ```

### Usage

Run the project using:

```bash
cargo run
```

## Contributing

Contributions are welcome! Please read the [contributing guidelines](CONTRIBUTING.md) for more details.

## License

This project is licensed under the BSD-Clause-3 License - see the [LICENSE](LICENSE) file for details.

## Contact

For any questions, please contact Himanshu at hyattherate2005@gmail.com.

### Binary Installation

You can download pre-built binaries for your platform:

#### Linux

```bash
curl -LO https://github.com/Himasnhu-at/ruspy/releases/latest/download/ruspy-linux.tar.gz
tar xzf ruspy-linux.tar.gz
sudo mv ruspy-linux /usr/local/bin/ruspy
```

#### Windows

Download `ruspy-windows.zip` from the releases page and extract it.

#### macOS

```bash
curl -LO https://github.com/Himasnhu-at/ruspy/releases/latest/download/ruspy-macos.tar.gz
tar xzf ruspy-macos.tar.gz
sudo mv ruspy-macos /usr/local/bin/ruspy
```

6. To build the releases, run:

> [!NOTE]
> Before running the build script, make sure you have cross toolchain installed.
> MacOS build is only supported on macOS.
> Toolchains:
>
> - `x86_64-unknown-linux-musl`
> - `x86_64-pc-windows-gnu`
> - `aarch64-apple-darwin`
>
> To install, run:
>
> ```cargo
> rustup target add x86_64-unknown-linux-musl
> rustup target add x86_64-pc-windows-gnu
> rustup target add aarch64-apple-darwin
> ```

```bash
chmod +x build.sh
./build.sh
```

This setup will:

- Optimize the binary size and performance
- Create static binaries that don't require Rust installation
- Support cross-compilation for different platforms
- Package the binaries appropriately for each platform
