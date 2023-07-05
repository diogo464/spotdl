#!/bin/sh

if [ "$1" = "linux" ]; then
	rustup target install x86_64-unknown-linux-gnu || exit 1
	cargo build --target-dir target/ --target x86_64-unknown-linux-gnu --release || exit 1
	mv target/x86_64-unknown-linux-gnu/release/spotdl "spotdl_x86_64-unknown-linux-gnu"
fi

if [ "$1" = "macos" ]; then
	rustup target install x86_64-apple-darwin || exit 1
	cargo build --target-dir target/ --target x86_64-apple-darwin --release || exit 1
	mv target/x86_64-apple-darwin/release/spotdl "spotdl_x86_64-apple-darwin"
fi
