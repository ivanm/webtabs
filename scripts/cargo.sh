#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null && [[ -x target/toolchain/cargo/bin/cargo ]]; then
    export CARGO_HOME="$PWD/target/toolchain/cargo"
    export RUSTUP_HOME="$PWD/target/toolchain/rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi
exec cargo "$@"
