PREFIX ?= $(HOME)/.local
GROK_BUILD ?= $(CURDIR)/../grok-build

.PHONY: inject build install smoke

inject:
	python3 scripts/inject.py "$(CURDIR)" "$(GROK_BUILD)"

build: inject
	cd "$(GROK_BUILD)" && cargo build -p palmbridge

install:
	./install.sh

smoke: build
	"$(GROK_BUILD)/target/debug/palmbridge" --version
	"$(GROK_BUILD)/target/debug/palmbridge" --help
	"$(GROK_BUILD)/target/debug/palmbridge" status --json >/dev/null
