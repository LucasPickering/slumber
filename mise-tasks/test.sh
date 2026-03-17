#!/bin/sh
#MISE description="Run tests via Cargo"
# 
#USAGE flag "--package -p <package>" help="Crate(s) to run tests in" {
#USAGE   choices "cli" "config" "core" "fs" "import" "macros" "python" "template" "tui" "util"
#USAGE }
#USAGE flag "--backtrace --bt" help="Enable RUST_BACKTRACE"
#USAGE arg "[rest]" var=#true help="Additional arguments to pass to the test binary"

args="--lib"
if [ -z "$usage_package" ]; then
  args="$args --workspace --all-features"
else
  for crate in $usage_package; do
    args="$args -p slumber_$crate"
  done
fi

if [ -z $usage_backtrace ]; then
  export RUST_BACKTRACE=0
else
  export RUST_BACKTRACE=1
fi 
set -x
exec cargo test $args -- $usage_rest
