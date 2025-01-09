# Ruspy

Ruspy is a project that aims to combine the efficiency of Rust with the simplicity and readability of Python. This project is designed to provide a syntax that is easy to learn and use, while maintaining high performance.

> [!IMPORTANT]
> This is concept language compiler, not suggested for production use.
> Just wanted to learn compiler design along with rust

## Features

- Rust-like performance
- Python-like syntax
- Easy to learn and use

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
