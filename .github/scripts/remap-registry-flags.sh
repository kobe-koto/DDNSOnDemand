#!/bin/sh
# Print rustc flags that strip the cargo registry source directory from the
# file paths embedded in the binary. The paths come from panic locations and
# tracing callsite metadata. For example
#   /usr/local/cargo/registry/src/index.crates.io-<hash>/tokio-1.54.1/src/...
# becomes
#   tokio-1.54.1/src/...
# There are ~340 such paths, so this saves ~20-30 KB, and the binary no longer
# contains build-host paths. Log output is unaffected: the fmt layer does not
# print file paths.
#
# Run it after `cargo fetch`, so the directories exist, and append its output
# to RUSTFLAGS. It emits one flag per registry index directory. The directory
# name carries a hash that can change between cargo versions, so it is looked
# up, not hard-coded.
#
# POSIX sh: the musl job runs it under busybox in rust:alpine.
set -eu

found=0
for dir in "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/; do
    if [ -d "$dir" ]; then
        printf ' --remap-path-prefix=%s=' "$dir"
        found=1
    fi
done

if [ "$found" -eq 0 ]; then
    echo "warning: no cargo registry sources found; paths will not be remapped" >&2
fi
