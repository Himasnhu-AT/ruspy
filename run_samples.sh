#!/bin/bash

# Set RUST_LOG environment variable for detailed logging if needed
export RUST_LOG=debug

# Build the project
echo "Building the project..."
cargo build

# Run the example file with debug mode
echo "Running sample.ruspy with debug mode..."
cargo run -- -d examples/sample.ruspy

# Run the example file without debug mode
echo "Running sample.ruspy without debug mode..."
cargo run -- examples/sample.ruspy

# Make this script executable with: chmod +x run_samples.sh
