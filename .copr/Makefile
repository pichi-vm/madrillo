# Copr SCM entry point. The source is the committed tree; scratch files,
# credentials, test results and build products cannot enter the source archive.
SHELL := /bin/bash
DNF ?= dnf
outdir ?= $(CURDIR)
spec ?= packaging/madrillo.spec

.PHONY: srpm
srpm:
	$(DNF) -y install cargo rust rpm-build tar xz gzip git
	set -euo pipefail; \
	root=$$(git rev-parse --show-toplevel); \
	workdir=$$(mktemp -d); \
	trap 'rm -rf "$$workdir"' EXIT; \
	mkdir -p "$(outdir)" "$$workdir/source"; \
	output=$$(realpath "$(outdir)"); \
	specfile="$(spec)"; \
	if [[ "$$specfile" != /* ]]; then specfile="$$root/$$specfile"; fi; \
	name=$$(awk '/^Name:/ {print $$2; exit}' "$$specfile"); \
	version=$$(awk '/^Version:/ {print $$2; exit}' "$$specfile"); \
	git -C "$$root" archive HEAD | tar -C "$$workdir/source" -xf -; \
	cd "$$workdir/source"; \
	cargo vendor --locked --versioned-dirs vendor > /dev/null; \
	tar -cJf "$$workdir/$$name-$$version-vendor.tar.xz" vendor; \
	rm -rf vendor; \
	tar -czf "$$workdir/$$name-$$version.tar.gz" \
		--transform "s,^\./,$$name-$$version/," .; \
	rpmbuild -bs "$$specfile" \
		--define "_sourcedir $$workdir" \
		--define "_srcrpmdir $$output" \
		--define "_disable_source_fetch 1"
