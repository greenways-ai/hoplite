VERSION ?= $(shell grep -m1 '^version = ' ../../Cargo.toml | cut -d'"' -f2)
FORMULA ?= Formula/hoplite.rb

.PHONY: formula audit install-test

formula:
	@test -n "$(ARM64_SHA256)" || (printf '%s\n' 'ARM64_SHA256 is required' && exit 1)
	@test -n "$(X86_64_SHA256)" || (printf '%s\n' 'X86_64_SHA256 is required' && exit 1)
	mkdir -p Formula
	VERSION=$(VERSION) ARM64_SHA256=$(ARM64_SHA256) X86_64_SHA256=$(X86_64_SHA256) \
		../../scripts/render-homebrew-formula > $(FORMULA)

audit:
	brew audit --strict --online $(FORMULA)

install-test:
	brew install --build-from-source $(FORMULA)
	brew test hoplite
