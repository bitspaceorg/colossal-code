#!/bin/bash
# Test script to verify Landlock sandbox is working

echo "=== Testing Sandbox Enforcement ==="
echo ""

# Build the project
echo "1. Building project..."
cargo build --release 2>&1 | tail -1

# Test 1: Should succeed - accessing current directory
echo ""
echo "2. Test: Access current directory (should SUCCEED)"
cd /home/wise/rust/tool_agent
cargo run --release 2>&1 | grep -i "success\|error" | head -5

# Test 2: Should fail - accessing unauthorized directory
echo ""
echo "3. Test: Access /home/wise/arsenal/age without SANDBOX_EXTRA_ROOTS (should FAIL)"
# This should fail because /home/wise/arsenal/age is not in writable_roots
export SANDBOX_EXTRA_ROOTS=""
# Add test command here that tries to access /home/wise/arsenal/age

echo ""
echo "4. Test: Access /home/wise/arsenal/age WITH SANDBOX_EXTRA_ROOTS (should SUCCEED)"
export SANDBOX_EXTRA_ROOTS="/home/wise/arsenal/age"
# Add test command here that tries to access /home/wise/arsenal/age

echo ""
echo "=== Sandbox Test Complete ==="
echo ""
echo "Manual verification steps:"
echo "1. Run: cargo run --release"
echo "2. Try to read a file outside the workspace"
echo "3. You should get 'Permission denied' errors"
