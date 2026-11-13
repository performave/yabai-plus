FRAMEWORK_PATH = -F/System/Library/PrivateFrameworks
FRAMEWORK      = -framework Carbon -framework Cocoa -framework CoreServices -framework CoreVideo -framework SkyLight
CLI_FLAGS      =
VERSION        ?= $(shell git describe --tags --exact-match 2>/dev/null)
ifneq ($(VERSION),)
CLI_FLAGS      += -DYABAI_VERSION='"$(VERSION)"'
endif
BUILD_FLAGS    = -std=c11 -Wall -Wextra -g -O0 -fvisibility=hidden -mmacosx-version-min=11.0 -fno-objc-arc -arch x86_64 -arch arm64 -sectcreate __TEXT __info_plist $(INFO_PLIST)
BUILD_PATH     = ./bin
DOC_PATH       = ./doc
SCRIPT_PATH    = ./scripts
ASSET_PATH     = ./assets
SMP_PATH       = ./examples
ARCH_PATH      = ./archive
OSAX_SRC       = ./src/osax/payload_bin.c ./src/osax/loader_bin.c
YABAI_SRC      = ./src/manifest.m $(OSAX_SRC)
OSAX_PATH      = ./src/osax
INFO_PLIST     = $(ASSET_PATH)/Info.plist
BINS           = $(BUILD_PATH)/yabai

.PHONY: all asan tsan install man icon archive publish sign clean-build clean

all: clean-build $(BINS)

asan: BUILD_FLAGS=-std=c11 -Wall -Wextra -g -O0 -fvisibility=hidden -fsanitize=address,undefined -mmacosx-version-min=11.0 -fno-objc-arc -arch x86_64 -arch arm64 -sectcreate __TEXT __info_plist $(INFO_PLIST)
asan: clean-build $(BINS)

tsan: BUILD_FLAGS=-std=c11 -Wall -Wextra -g -O0 -fvisibility=hidden -fsanitize=thread,undefined -mmacosx-version-min=11.0 -fno-objc-arc -arch x86_64 -arch arm64 -sectcreate __TEXT __info_plist $(INFO_PLIST)
tsan: clean-build $(BINS)

install: BUILD_FLAGS=-std=c11 -Wall -Wextra -DNDEBUG -O3 -fvisibility=hidden -mmacosx-version-min=11.0 -fno-objc-arc -arch x86_64 -arch arm64 -sectcreate __TEXT __info_plist $(INFO_PLIST)
install: clean-build $(BINS)

$(OSAX_SRC): $(OSAX_PATH)/loader.m $(OSAX_PATH)/payload.m
	xcrun clang $(OSAX_PATH)/payload.m -shared -fPIC -O3 -mmacosx-version-min=11.0 -arch x86_64 -arch arm64e -o $(OSAX_PATH)/payload $(FRAMEWORK_PATH) -framework SkyLight -framework Foundation -framework Carbon
	xcrun clang $(OSAX_PATH)/loader.m -O3 -mmacosx-version-min=11.0 -arch x86_64 -arch arm64e -o $(OSAX_PATH)/loader -framework Cocoa
	xxd -i -a $(OSAX_PATH)/payload $(OSAX_PATH)/payload_bin.c
	xxd -i -a $(OSAX_PATH)/loader $(OSAX_PATH)/loader_bin.c
	rm -f $(OSAX_PATH)/payload
	rm -f $(OSAX_PATH)/loader

man:
	asciidoctor -b manpage $(DOC_PATH)/yabai.asciidoc -o $(DOC_PATH)/yabai.1

icon:
	python3 $(SCRIPT_PATH)/seticon.py $(ASSET_PATH)/icon/2x/icon-512px@2x.png $(BUILD_PATH)/yabai

publish:
	sed -i '' "s/^VERSION=.*/VERSION=\"$(shell $(BUILD_PATH)/yabai --version | cut -d "v" -f 2)\"/" $(SCRIPT_PATH)/install.sh

archive: man install sign icon
	rm -rf $(ARCH_PATH)
	mkdir -p $(ARCH_PATH)
	cp -r $(BUILD_PATH) $(ARCH_PATH)/
	cp -r $(DOC_PATH) $(ARCH_PATH)/
	cp -r $(SMP_PATH) $(ARCH_PATH)/
	tar -cvzf $(BUILD_PATH)/$(shell $(BUILD_PATH)/yabai --version).tar.gz $(ARCH_PATH)
	rm -rf $(ARCH_PATH)

sign:
	codesign -fs "yabai-cert" $(BUILD_PATH)/yabai

clean-build:
	rm -rf $(BUILD_PATH)

clean: clean-build
	rm -f $(OSAX_SRC)

$(BUILD_PATH)/yabai: $(YABAI_SRC)
	mkdir -p $(BUILD_PATH)
	xcrun clang $^ $(BUILD_FLAGS) $(CLI_FLAGS) $(FRAMEWORK_PATH) $(FRAMEWORK) -o $@

# ============================================================================
# yabai-plus local-dev additions (no equivalent upstream).
#
# Everything for this fork's local workflow lives in this one block, with its
# own .PHONY line and variables, so it only ever ADDS lines at the end of the
# file. Rebases onto upstream don't touch these lines -> no conflicts. Don't
# fold any of this into the upstream targets above.
# ============================================================================
.PHONY: build release dev dev-restore sa-status

# Signing identity for the dev binary (overridable: `make dev DEV_IDENTITY="..."`).
# Any real certificate keeps the binary's designated requirement stable, so macOS
# preserves its Accessibility grant + scripting-addition trust across rebuilds; it
# need not be a Developer ID. This is the free account's Apple Development
# certificate, the same one rift-plus's justfile uses. It is named rather than
# auto-detected: the keychain still holds the paid team's revoked Developer ID,
# which codesign signs with happily and macOS then kills at launch.
DEV_IDENTITY   ?= Apple Development: eric.wang401@gmail.com (2RTC9978WY)
DEV_DEST       ?= /opt/homebrew/bin/yabai

# Canary version string baked into the dev binary so `yabai --version` makes it
# obvious you're running a locally-built swap and not the Homebrew release.
# git describe -> nearest tag + commits-since + short sha (+ "-dirty" for an
# uncommitted tree), then a "-canary" marker; falls back to the bare sha if there
# is no tag in reach. This flows into -DYABAI_VERSION via the VERSION plumbing at
# the top of the file (which is why `dev` re-invokes make with VERSION set).
DEV_VERSION    ?= $(shell git describe --tags --dirty --always 2>/dev/null)-canary

# Passwordless-sudo rule for `yabai --load-sa`. The launchd service has no tty to type
# a sudo password at, so without this the scripting addition never injects and window
# moves fall back to blocking AX (the jarring mid-drag freeze). The rule is pinned to the
# binary's sha256, which changes every build, so `dev` regenerates it each time.
DEV_SUDOERS    ?= /private/etc/sudoers.d/yabai

# Friendly aliases for upstream's confusingly-named build targets.
build: all       # debug build       -> bin/yabai (upstream: `all`)
release: install # optimized -O3 build -> bin/yabai (upstream: `install`, installs nothing)

# Build, sign with DEV_IDENTITY (so the Accessibility grant + scripting-addition
# trust carry over), swap into the Homebrew path in place, and restart the service.
# The scripting addition lives in Dock and survives yabai restarts, so no --load-sa
# is needed here unless Dock itself has restarted.
# DEV_DEST is a Homebrew symlink into the (user-writable) Cellar. We keep the symlink
# and overwrite the file it points at IN PLACE, rather than replacing the symlink with
# a literal file. macOS ties a CLI tool's Accessibility grant to the *resolved* binary
# path, so reusing the exact path production runs from avoids re-granting Accessibility
# on every dev swap. rm-then-cp gives a fresh inode at the same path, which both dodges
# "can't overwrite a running binary" (ETXTBSY) and leaves the path unchanged.
# Re-invoke make with VERSION set so the canary string is baked in at parse time
# (the -DYABAI_VERSION conditional at the top is evaluated when the makefile is
# read, not per-recipe, so a target-specific override wouldn't reach it).
dev:
	$(MAKE) all VERSION="$(DEV_VERSION)"
	codesign --force --options runtime --sign "$(DEV_IDENTITY)" $(BUILD_PATH)/yabai
	codesign --verify --strict $(BUILD_PATH)/yabai
	@target="$$(readlink -f $(DEV_DEST) 2>/dev/null || python3 -c 'import os,sys;print(os.path.realpath(sys.argv[1]))' $(DEV_DEST))"; \
	if [ "$$target" != "$(DEV_DEST)" ] && [ ! -e "$$target.brew-orig" ]; then cp "$$target" "$$target.brew-orig"; fi; \
	rm -f "$$target"; \
	cp $(BUILD_PATH)/yabai "$$target"
	# Regenerate the sha256-pinned passwordless --load-sa rule for the new binary. Write to
	# a temp file and validate it with `visudo -cf` before moving it into place, so a bad
	# line can never lock sudo. With the rule in place, the service's own `sudo yabai
	# --load-sa` (and the explicit one below) run without a password and the SA injects.
	echo "$$(id -un) ALL=(root) NOPASSWD: sha256:$$(shasum -a 256 $(DEV_DEST) | cut -d ' ' -f1) $(DEV_DEST) --load-sa" | sudo tee $(DEV_SUDOERS).tmp >/dev/null
	sudo chmod 0440 $(DEV_SUDOERS).tmp
	sudo visudo -cf $(DEV_SUDOERS).tmp
	sudo mv $(DEV_SUDOERS).tmp $(DEV_SUDOERS)
	yabai --restart-service || yabai --start-service
	# Best-effort SA injection. It can fail for environment reasons (macOS hardening of the
	# Dock injection path, Dock in a bad state, SIP/boot-arg) -- that only means window moves
	# use the slower AX path, so don't fail the whole build over it.
	sudo $(DEV_DEST) --load-sa || echo ">> warning: SA injection failed; window moves will use AX. Try: killall Dock && sudo $(DEV_DEST) --load-sa && yabai --check-sa"
	@echo "swapped in $$($(DEV_DEST) --version); SA status: $$($(DEV_DEST) --check-sa)"

# Report whether the scripting addition is live. The payload running inside Dock creates
# this socket on inject; its presence is the most reliable non-invasive signal. If it's
# missing, window moves fall back to blocking AX (the mid-drag freeze).
sa-status:
	@if [ -S /tmp/yabai-sa_$$(id -un).socket ]; then \
		echo "scripting-addition: LOADED (/tmp/yabai-sa_$$(id -un).socket present)"; \
	else \
		echo "scripting-addition: NOT loaded -- run 'sudo $(DEV_DEST) --load-sa'"; \
	fi

# Restore the genuine Homebrew binary. `make dev` overwrites the Cellar file in place
# and backs the original up as <file>.brew-orig; put that back (same rm-then-cp, so the
# resolved path -- and thus the Accessibility grant -- is preserved). Fall back to
# re-linking the formula if no backup is present (e.g. an older swap replaced the symlink).
dev-restore:
	@target="$$(readlink -f $(DEV_DEST) 2>/dev/null || python3 -c 'import os,sys;print(os.path.realpath(sys.argv[1]))' $(DEV_DEST))"; \
	if [ -e "$$target.brew-orig" ]; then \
		rm -f "$$target"; cp "$$target.brew-orig" "$$target"; rm -f "$$target.brew-orig"; \
		echo "restored genuine brew binary at $$target"; \
	else \
		brew unlink yabai-plus && brew link --overwrite yabai-plus; \
	fi
	yabai --restart-service
	@echo "restored $$($(DEV_DEST) --version) and restarted"
