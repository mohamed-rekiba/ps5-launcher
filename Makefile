# The project's common tasks. Each target runs the same commands CI and the docs use; nothing
# here is a second build system. Run `make` to list them.
#
# Shell tests and lints that need Linux tools run in containers, so they also work on macOS.

.DEFAULT_GOAL := help
SHELL := bash

DOCKER ?= docker
FEDORA_VERSION ?= $(shell bash -c 'source packaging/os/config.sh; printf "%s" "$$FEDORA_VERSION"')
FEDORA ?= quay.io/fedora/fedora:$(FEDORA_VERSION)
export FEDORA_VERSION FEDORA_ISO_VERSION FEDORA_GPG_FINGERPRINT
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
# The binary the Linux packages wrap (a Linux x86_64 build).
BINARY ?= target/release/ps5-launcher
DIST ?= dist/packages

# Every shell script, chosen by its #! line (libexec also has a Python helper).
SHELL_SCRIPTS := $(shell grep -lE '^\#!.*(ba)?sh' install.sh scripts/*.sh packaging/linux/*.sh \
	packaging/linux/ps5-launcher-session packaging/os/*.sh packaging/os/boottest/* \
	packaging/os/files/usr/libexec/*/* 2>/dev/null)
OS_TESTS := packaging/os/test-config.sh packaging/os/test-helper.sh packaging/os/test-boot-health.sh packaging/os/test-recovery-menu.sh \
	packaging/os/test-signature-policy.sh

.PHONY: help build run run-session test check check-shell lint packages appimage os-image os-iso clean

help: ## List the targets
	@grep -E '^[a-z-]+:.*## ' $(MAKEFILE_LIST) | awk -F':.*## ' '{printf "  %-12s %s\n", $$1, $$2}'

build: ## Build the release binary
	cargo build --release --locked

run: ## Run the launcher in a window (desktop mode)
	cargo run --release --locked -- --windowed

run-session: ## Run it as in the PS5 Launcher session, in a window: the System pages act on this PC
	PS5_LAUNCHER_SESSION=1 cargo run --release --locked -- --windowed

test: ## Run the Rust tests
	cargo test --profile ci --locked

check: ## Everything CI's Linux job checks: build, tests, smoke test, shell tests
	cargo build --profile ci --locked
	cargo test --profile ci --locked
	scripts/smoke-test.sh target/ci/ps5-launcher
	$(MAKE) check-shell

check-shell: ## The session wrapper's tests here, and the OS tests in the selected Fedora release
	packaging/linux/test-session.sh
	$(DOCKER) run --rm -v "$(CURDIR):/repo:ro" -w /repo -e FEDORA_VERSION $(FEDORA) bash -o pipefail -c '\
		dnf -y -q install jq python3 util-linux procps-ng findutils diffutils >/dev/null && \
		for t in $(OS_TESTS); do echo "== $$t"; bash "$$t" | tail -1 || exit 1; done'

lint: ## shellcheck on every script, actionlint on the workflows
	shellcheck $(SHELL_SCRIPTS)
	$(DOCKER) run --rm -v "$(CURDIR):/repo" -w /repo rhysd/actionlint:latest -no-color

packages: ## deb, rpm and Arch packages of BINARY (a Linux build), with nfpm in a container
	mkdir -p $(DIST)
	for p in deb rpm archlinux; do \
		$(DOCKER) run --rm -v "$(CURDIR):/src" -w /src -e VERSION=$(VERSION) -e BINARY=$(BINARY) \
			goreleaser/nfpm:v2.47.0 package -f packaging/linux/nfpm.yaml -p $$p -t $(DIST) || exit 1; \
	done

appimage: ## The AppImage of BINARY
	packaging/linux/build-appimage.sh $(BINARY) $(DIST)

os-image: ## The OS main image; needs LAUNCHER_RPM (see packaging/os/README.md)
	@test -n "$(LAUNCHER_RPM)" || { echo "Set LAUNCHER_RPM=path/to/ps5-launcher-*.rpm (make packages makes one)"; exit 1; }
	LAUNCHER_RPM=$(LAUNCHER_RPM) packaging/os/build-image.sh main

os-iso: ## The installer ISO for IMAGE (see packaging/os/build-iso.sh)
	@test -n "$(IMAGE)" || { echo "Set IMAGE=ghcr.io/OWNER/ps5-launcher-fedora"; exit 1; }
	$(DOCKER) run --rm --privileged -v "$(CURDIR):/src" -w /src -e IMAGE="$(IMAGE)" -e MAIN_DIGEST="$(MAIN_DIGEST)" \
		-e FEDORA_VERSION -e FEDORA_ISO_VERSION -e FEDORA_GPG_FINGERPRINT $(FEDORA) \
		packaging/os/build-iso.sh ps5-launcher-fedora-x86_64.iso

clean: ## Remove build output (target/, dist/)
	cargo clean
	rm -rf dist
