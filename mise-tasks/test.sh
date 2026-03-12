
#!/bin/sh
#MISE description="Run tests via Cargo"
#MISE env = {"RUST_BACKTRACE" = 0}
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

export RUST_BACKTRACE=$usage_backtrace
set -x
exec cargo test $args -- $usage_test
