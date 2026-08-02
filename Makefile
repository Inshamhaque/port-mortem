# port-mortem build harness for cjson-rs (safe Rust port of cJSON v1.7.19).
#
# Targets:
#   make build           cargo build --release (lib + staticlib + CLI)
#   make test            cargo test (Rust unit + integration tests)
#   make test-original   compile+run the UNMODIFIED original C test suite
#                        against the Rust FFI layer (libcjson_rs.a)
#   make verify          everything above
#   make fuzz            differential fuzzer (FFI vs safe core), 60s run
#   make bench           benchmark + write bench/results.json
#   make clean

CARGO := cargo
CC    ?= cc

STAGING   := build/original-tests
LIB       := target/release/libcjson_rs.a
CFLAGS    := -std=c11 -O1 -Wall
CJSON_LIB := -L target/release -lcjson_rs -lm

# The 18 library tests + 3 utils tests from tests/original/CMakeLists.txt.
UNITY_TESTS := parse_examples parse_number parse_hex4 parse_string parse_array \
	parse_object parse_value print_string print_number print_array print_object \
	print_value misc_tests parse_with_opts compare_tests cjson_add readme_examples \
	minify_tests
UTILS_TESTS := json_patch_tests old_utils_tests misc_utils_tests
ALL_TESTS   := $(UNITY_TESTS) $(UTILS_TESTS)

.PHONY: all build test test-original verify fuzz bench clean

all: build

build:
	$(CARGO) build --release

test:
	$(CARGO) test

$(LIB): build

# Compile and run the original test suite. Each test binary is the original
# .c + Unity, linked against the Rust staticlib; fixtures are staged like the
# original CMake does (inputs/ and json-patch-tests/ are copied into the cwd
# the tests run from).
test-original: $(LIB)
	@rm -rf $(STAGING)
	@mkdir -p $(STAGING)/bin
	@cp -R tests/original/inputs $(STAGING)/inputs
	@cp -R tests/original/json-patch-tests $(STAGING)/json-patch-tests
	@set -e; \
	fail=0; \
	for t in $(ALL_TESTS); do \
		echo "== building $$t =="; \
		$(CC) $(CFLAGS) -I tests/original tests/original/$$t.c \
			tests/original/unity/src/unity.c $(CJSON_LIB) -o $(STAGING)/bin/$$t \
			|| { echo "BUILD FAILED: $$t"; fail=1; break; }; \
	done; \
	if [ $$fail -eq 0 ]; then \
		total=0; passed=0; \
		for t in $(ALL_TESTS); do \
			total=$$((total+1)); \
			if ( cd $(STAGING) && ./bin/$$t ) > $(STAGING)/$$t.log 2>&1; then \
				passed=$$((passed+1)); \
				echo "PASS $$t  ($$(tail -1 $(STAGING)/$$t.log))"; \
			else \
				fail=1; \
				echo "FAIL $$t"; \
				cat $(STAGING)/$$t.log; \
			fi; \
		done; \
		echo "== original suite: $$passed/$$total passed =="; \
	fi; \
	exit $$fail

verify: test test-original

fuzz: build
	$(CC) $(CFLAGS) -I tests fuzz/driver.c $(CJSON_LIB) -o build/fuzz-driver
	python3 fuzz/harness.py --duration 60 --log fuzz/log.txt \
		--driver build/fuzz-driver --cli target/release/cjson-rs

bench: build
	python3 bench/run.py

clean:
	rm -rf build
	$(CARGO) clean
