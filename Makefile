.PHONY: test fuzz examples verify release

test:
	cargo test --offline

examples:
	cargo run --quiet -- run examples/hello.ae
	cargo run --quiet -- run examples/fib.ae
	cargo run --quiet -- run examples/opaque.ae -O2
	cargo run --quiet -- run stdlib/math.ae
	cargo run --quiet -- optimize examples/opt_demo.ae
	cargo run --quiet -- verify examples/fib.ae
	cargo run --quiet -- fmt examples/hello.ae

fuzz:
	cargo run --quiet -- fuzz --iters 40 --kind all --seed 1

verify:
	cargo run --quiet -- verify examples/opt_demo.ae -O2

release:
	cargo build --release
