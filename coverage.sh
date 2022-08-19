#!/bin/sh

cargo llvm-cov nextest -j 10 --lcov --output-path lcov.info

