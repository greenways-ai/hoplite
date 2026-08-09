NGINX_VERSION := 1.30.4
NGINX_SHA256 := 4261dc90e9e47c1c4041276e9aaa3d48ebe2e664f728e14fa95ae6c67d57a08b
SOURCE_ROOT := $(abspath $(CURDIR)/target/sources)
NGINX_ARCHIVE := $(SOURCE_ROOT)/nginx-$(NGINX_VERSION).tar.gz
DEFAULT_NGINX_SRC := $(SOURCE_ROOT)/nginx-$(NGINX_VERSION)
NGINX_SRC ?= $(DEFAULT_NGINX_SRC)
RUNTIME_DIR := $(CURDIR)/target/release
VALUE_STORE_PROVIDER_MANIFEST := $(CURDIR)/abi/value-store-provider-ffi/Cargo.toml
VALUE_STORE_PROVIDER_DIR := $(CURDIR)/abi/value-store-provider-ffi/target/release
VALUE_PROVIDER_MANIFEST := $(CURDIR)/abi/value-provider-filesystem-ffi/Cargo.toml
VALUE_PROVIDER_DIR := $(CURDIR)/abi/value-provider-filesystem-ffi/target/release
BLOB_STORE_PROVIDER_MANIFEST := $(CURDIR)/abi/blob-store-provider-ffi/Cargo.toml
BLOB_STORE_PROVIDER_DIR := $(CURDIR)/abi/blob-store-provider-ffi/target/release
UNAME_S := $(shell uname -s)
HOPLITE_VERSION := $(shell grep -m1 '^version = ' $(CURDIR)/Cargo.toml | cut -d'"' -f2)
HOST_TARGET ?= $(shell rustc -vV | sed -n 's/^host: //p')
STRIP ?= strip

ifeq ($(UNAME_S),Darwin)
STRIP_FLAGS ?= -S -x
else
STRIP_FLAGS ?= --strip-unneeded
endif

PACKAGE_ROOT := $(abspath $(CURDIR)/target/dist)
HOPLITE_ARTIFACT := $(PACKAGE_ROOT)/hoplite-v$(HOPLITE_VERSION)-$(HOST_TARGET)
HOPLITE_SERVER_ARTIFACT := $(PACKAGE_ROOT)/hoplite-server-v$(HOPLITE_VERSION)-$(HOST_TARGET)

ifeq ($(UNAME_S),Darwin)
OPENSSL_PREFIX ?= $(shell brew --prefix openssl@3)
PCRE2_PREFIX ?= $(shell brew --prefix pcre2)
NGINX_CC_OPT ?= -I$(OPENSSL_PREFIX)/include -I$(PCRE2_PREFIX)/include
NGINX_LD_OPT ?= $(PCRE2_PREFIX)/lib/libpcre2-8.a $(OPENSSL_PREFIX)/lib/libssl.a $(OPENSSL_PREFIX)/lib/libcrypto.a -Wl,-dead_strip_dylibs
endif

.PHONY: help setup check runtime value-store-provider value-provider blob-store-provider cli embedded-cli server-cli test nginx nginx-source docker \
	check-macos macos macos-package osx install uninstall example-check \
	example-build example-dev example-prod benchmark-bytecode

PREFIX ?= /usr/local

help:
	@printf '%s\n' \
		'Hoplite development' \
		'  make setup              Install macOS build dependencies and fetch crates' \
		'  make check              Run the complete Rust workspace test suite' \
		'  make embedded-cli       Build the all-in-one CLI and production server' \
		'  make server-cli         Build the slim production server' \
		'  make macos              Build standalone macOS executables' \
		'  make install            Install Hoplite and hoplite-server under PREFIX' \
		'  make example-dev        Build and start the example in dev mode' \
		'  make example-prod       Build and start the example in prod mode' \
		'  make benchmark-bytecode Compare HAL and HBC library loading'

setup: check-macos
	brew install rust openssl@3 pcre2
	cargo fetch --manifest-path Cargo.toml
	cargo fetch --manifest-path runtime/Cargo.toml
	cargo fetch --manifest-path $(VALUE_STORE_PROVIDER_MANIFEST)
	cargo fetch --manifest-path $(VALUE_PROVIDER_MANIFEST)
	cargo fetch --manifest-path $(BLOB_STORE_PROVIDER_MANIFEST) --locked

check:
	cargo test --workspace

runtime:
	cargo build --manifest-path runtime/Cargo.toml --release

# These crates are intentionally standalone FFI workspaces. Their path
# dependencies are supplied by the reviewed source checkout used for the build.
value-store-provider:
	cargo build --manifest-path $(VALUE_STORE_PROVIDER_MANIFEST) --release

value-provider:
	cargo build --manifest-path $(VALUE_PROVIDER_MANIFEST) --release

blob-store-provider:
	cargo build --manifest-path $(BLOB_STORE_PROVIDER_MANIFEST) --release --locked

cli:
	cargo build --manifest-path Cargo.toml --release --bin hoplite

embedded-cli: server-cli
	cargo build --manifest-path Cargo.toml --release \
		--features embedded-nginx --bin hoplite
	$(STRIP) $(STRIP_FLAGS) target/release/hoplite
	mkdir -p $(PACKAGE_ROOT)
	install -m 755 target/release/hoplite $(HOPLITE_ARTIFACT)

server-cli: nginx
	cargo build --manifest-path Cargo.toml --release \
		--features embedded-nginx --bin hoplite-server
	$(STRIP) $(STRIP_FLAGS) target/release/hoplite-server
	mkdir -p $(PACKAGE_ROOT)
	install -m 755 target/release/hoplite-server $(HOPLITE_SERVER_ARTIFACT)

test:
	cargo test --manifest-path runtime/Cargo.toml

example-check:
	cargo run -- serve check --mode dev examples

example-build:
	cargo run -- serve build --mode dev examples

example-dev:
	cargo run -- serve foreground --mode dev examples

example-prod:
	cargo run --release -- serve foreground --mode prod examples

benchmark-bytecode:
	cargo run --release --bin hoplite-bytecode-loading -- 1000

$(NGINX_ARCHIVE):
	mkdir -p $(SOURCE_ROOT)
	curl --fail --location --retry 3 \
		https://nginx.org/download/nginx-$(NGINX_VERSION).tar.gz \
		--output $@.tmp
	echo "$(NGINX_SHA256)  $@.tmp" | shasum -a 256 --check
	mv $@.tmp $@

$(DEFAULT_NGINX_SRC)/configure: $(NGINX_ARCHIVE)
	tar -xzf $(NGINX_ARCHIVE) -C $(SOURCE_ROOT)

nginx-source: $(NGINX_SRC)/configure

nginx: runtime value-store-provider value-provider blob-store-provider nginx-source
	cd $(NGINX_SRC) && \
		HOPLITE_RUNTIME_DIR=$(RUNTIME_DIR) \
		HOPLITE_VALUE_STORE_PROVIDER_DIR=$(VALUE_STORE_PROVIDER_DIR) \
		HOPLITE_VALUE_PROVIDER_DIR=$(VALUE_PROVIDER_DIR) \
		HOPLITE_BLOB_STORE_PROVIDER_DIR=$(BLOB_STORE_PROVIDER_DIR) \
		./configure --prefix=$(CURDIR)/target/nginx \
			--with-http_ssl_module --with-threads \
			$(if $(NGINX_CC_OPT),--with-cc-opt="$(NGINX_CC_OPT)") \
			$(if $(NGINX_LD_OPT),--with-ld-opt="$(NGINX_LD_OPT)") \
			--add-module=$(CURDIR)/nginx && \
		$(MAKE) && $(MAKE) install
	$(STRIP) $(STRIP_FLAGS) target/nginx/sbin/nginx

docker:
	docker build -f ../packaging/docker/Dockerfile -t hoplite ../..

check-macos:
	@test "$(UNAME_S)" = "Darwin" || \
		(echo "The macos target must run on macOS" && exit 1)

macos-package: check-macos embedded-cli
	shasum -a 256 $(HOPLITE_ARTIFACT) $(HOPLITE_SERVER_ARTIFACT)
	@echo
	@echo "Standalone macOS executables ready:"
	@echo "  $(HOPLITE_ARTIFACT)"
	@echo "  $(HOPLITE_SERVER_ARTIFACT)"
	@echo
	@echo "Install them:"
	@echo "  install -m 755 $(HOPLITE_ARTIFACT) /usr/local/bin/hoplite"
	@echo "  install -m 755 $(HOPLITE_SERVER_ARTIFACT) /usr/local/bin/hoplite-server"
	@echo "  hoplite version"
	@echo "  hoplite-server version"
	@echo
	@echo "Install a project as a per-user launchd service:"
	@echo "  hoplite serve install /path/to/hoplite/project"
	@echo "  hoplite serve status /path/to/hoplite/project"
	@echo "  hoplite serve uninstall /path/to/hoplite/project"

macos: macos-package

osx: macos

install: macos-package
	install -d $(DESTDIR)$(PREFIX)/bin
	install -m 755 $(HOPLITE_ARTIFACT) $(DESTDIR)$(PREFIX)/bin/hoplite
	install -m 755 $(HOPLITE_SERVER_ARTIFACT) $(DESTDIR)$(PREFIX)/bin/hoplite-server

uninstall:
	rm -f $(DESTDIR)$(PREFIX)/bin/hoplite $(DESTDIR)$(PREFIX)/bin/hoplite-server
