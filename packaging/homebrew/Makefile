VERSION ?= $(shell grep -m1 '^version = ' ../../core/Cargo.toml | cut -d'"' -f2)
HOPLITE_REVISION ?= $(shell git -C ../.. rev-parse HEAD 2>/dev/null)
HARA_REVISION ?= $(shell git -C ../../../hara rev-parse HEAD 2>/dev/null)
LICENSE ?= $(shell grep -m1 '^license = ' ../../core/Cargo.toml | cut -d'"' -f2)
NGINX_VERSION ?= $(shell sed -n 's/^NGINX_VERSION := //p' ../../core/Makefile)
NGINX_SHA256 ?= $(shell sed -n 's/^NGINX_SHA256 := //p' ../../core/Makefile)
FORMULA ?= Formula/hoplite.rb

.PHONY: formula check

formula:
	@test -n "$(VERSION)" || (printf '%s\n' 'VERSION is required' && exit 1)
	@test -n "$(HOPLITE_REVISION)" || (printf '%s\n' 'HOPLITE_REVISION is required' && exit 1)
	@test -n "$(HARA_REVISION)" || (printf '%s\n' 'HARA_REVISION is required' && exit 1)
	@test -n "$(LICENSE)" || (printf '%s\n' 'LICENSE is required' && exit 1)
	@test -n "$(NGINX_VERSION)" || (printf '%s\n' 'NGINX_VERSION is required' && exit 1)
	@test -n "$(NGINX_SHA256)" || (printf '%s\n' 'NGINX_SHA256 is required' && exit 1)
	mkdir -p $(dir $(FORMULA))
	VERSION=$(VERSION) \
	HOPLITE_REVISION=$(HOPLITE_REVISION) \
	HARA_REVISION=$(HARA_REVISION) \
	LICENSE=$(LICENSE) \
	NGINX_VERSION=$(NGINX_VERSION) \
	NGINX_SHA256=$(NGINX_SHA256) \
		../scripts/render-homebrew-formula > $(FORMULA)

check: formula
	ruby -c $(FORMULA)
