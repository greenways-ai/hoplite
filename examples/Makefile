HOPLITE ?= cargo run --manifest-path ../Cargo.toml --
MODE ?= dev
PROFILE ?= server

.PHONY: help check build start foreground stop restart status logs openapi clean

help:
	@printf '%s\n' \
		'Example Hoplite application' \
		'  make check               Validate the project and app resource' \
		'  make build               Generate .hoplite artifacts' \
		'  make start               Start nginx in the background' \
		'  make foreground          Run nginx in the foreground' \
		'  make restart             Rebuild and reload nginx' \
		'  make stop                Stop nginx' \
		'  make openapi             Print the generated OpenAPI document' \
		'Options: MODE=dev|prod PROFILE=server'

check:
	$(HOPLITE) serve check --mode $(MODE) --profile $(PROFILE) .

build:
	$(HOPLITE) serve build --mode $(MODE) --profile $(PROFILE) .

start:
	$(HOPLITE) serve start --mode $(MODE) --profile $(PROFILE) .

foreground:
	$(HOPLITE) serve foreground --mode $(MODE) --profile $(PROFILE) .

stop:
	$(HOPLITE) serve stop .

restart: build
	$(HOPLITE) serve reload .

status:
	$(HOPLITE) serve status .

logs:
	tail -n 100 .hoplite/error.log

openapi: build
	@printf '%s\n' '.hoplite/openapi/example.json'

clean:
	rm -rf .hoplite
