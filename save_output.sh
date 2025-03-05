#!/bin/bash

# Make sure the script is executable
chmod +x run_samples.sh

# Run the samples script and save output to file
./run_samples.sh > sampleOutput.txt 2>&1

echo "Output has been saved to sampleOutput.txt"

# Show a preview of the output
echo "Preview of output:"
echo "----------------"
head -n 20 sampleOutput.txt
echo "..."
echo "----------------"
echo "Full output saved to sampleOutput.txt"
