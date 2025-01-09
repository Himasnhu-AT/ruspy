#!/bin/bash

# Create release directory
mkdir -p releases

# Build for Linux (static binary)
echo "Building for Linux..."
cross build --target x86_64-unknown-linux-musl --release
cp target/x86_64-unknown-linux-musl/release/ruspy releases/ruspy-linux

# Build for Windows
echo "Building for Windows..."
cross build --target x86_64-pc-windows-gnu --release
cp target/x86_64-pc-windows-gnu/release/ruspy.exe releases/ruspy-windows.exe

# Build for M1 Mac (only on macOS)
if [[ "$OSTYPE" == "darwin"* ]]; then
    echo "Building for M1 Mac..."
    # Build natively instead of using cross
    cargo build --target aarch64-apple-darwin --release
    cp target/aarch64-apple-darwin/release/ruspy releases/ruspy-macos
else
    echo "Skipping macOS build (not on macOS)"
fi

# Create archives
cd releases
if [ -f ruspy-linux ]; then
    tar czf ruspy-linux.tar.gz ruspy-linux
fi

if [ -f ruspy-windows.exe ]; then
    zip ruspy-windows.zip ruspy-windows.exe
fi

if [ -f ruspy-macos ]; then
    tar czf ruspy-macos.tar.gz ruspy-macos
fi
cd ..
