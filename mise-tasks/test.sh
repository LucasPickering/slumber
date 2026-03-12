
#!/bin/sh
#MISE description="Run tests via Cargo"
# 
#USAGE arg "[crate]" var=#true help="Crate(s) to run tests in" {
#USAGE   choices "cli" "config" "core" "import" "macros" "python" "template" "tui" "util"
#USAGE }
#USAGE flag "--test -t <test>" var=#true help="Test(s) to run"
#USAGE flag "--backtrace --bt" help="Enable RUST_BACKTRACE"

args="--lib"
if [ -z "$usage_crate" ]; then
  args="$args --workspace --all-features"
else
  for crate in $usage_crate; do
    args="$args -p slumber_$crate"
  done
fi

if [ -z $usage_backtrace ]; then
  export RUST_BACKTRACE=0
else
  export RUST_BACKTRACE=1
fi 
set -x
exec cargo test $args -- $usage_test
