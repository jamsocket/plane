#!/bin/env bash
set -reuo pipefail
IFS=$'\n\t'
trap '__error_handling__' ERR
moldPath=1
clangPath=1

function __error_handling__() {
  if [[ $moldPath == "" ]]
  then
    >&2 echo "mold linker is not installed, please install it"
  fi

  if [[ $clangPath == "" ]]
  then
    >&2 echo "clang is not installed, please install it"
  fi
}

#check if mold and clang exist
moldPath=$(which mold)
clangPath=$(which clang)


export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="clang"
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS="-C link-arg=-fuse-ld=${moldPath}"

cargo build --features=full