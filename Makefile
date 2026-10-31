PREFIX ?= $(HOME)/.local
BIN     = $(PREFIX)/bin
APPS    = $(PREFIX)/share/applications
ICONS   = $(PREFIX)/share/icons/hicolor/512x512/apps
OLD_ICON = $(PREFIX)/share/icons/hicolor/scalable/apps/webtabs.svg
AUTOSTART = $(HOME)/.config/autostart
CARGO = bash scripts/cargo.sh
UNAME_S := $(shell uname -s)

.PHONY: build test install uninstall autostart run package package-arch

build:
	$(if $(filter Darwin,$(UNAME_S)),bash scripts/build-macos.sh,$(CARGO) build --release --locked)

test:
	$(CARGO) test --locked
	$(CARGO) clippy --all-targets -- -D warnings

run:
	$(if $(filter Darwin,$(UNAME_S)),$(MAKE) build && open target/release/WebTabs.app,$(CARGO) run)

install: build
ifeq ($(UNAME_S),Darwin)
	mkdir -p "$(HOME)/Applications"
	ditto target/release/WebTabs.app "$(HOME)/Applications/WebTabs.app"
else
	install -Dm755 target/release/webtabs $(BIN)/webtabs
	install -Dm644 packaging/webtabs.desktop $(APPS)/webtabs.desktop
	install -Dm644 assets/webtabs.png $(ICONS)/webtabs.png
	rm -f $(OLD_ICON)
	-update-desktop-database $(APPS) 2>/dev/null
	-gtk-update-icon-cache -q $(PREFIX)/share/icons/hicolor 2>/dev/null
endif

# Start hidden in the tray on login.
autostart: install
ifeq ($(UNAME_S),Darwin)
	@echo 'Enable Start at login in WebTabs Settings.'
else
	mkdir -p $(AUTOSTART)
	sed 's|^Exec=webtabs|Exec=$(BIN)/webtabs --hidden|' packaging/webtabs.desktop > $(AUTOSTART)/webtabs.desktop
endif

uninstall:
ifeq ($(UNAME_S),Darwin)
	rm -rf "$(HOME)/Applications/WebTabs.app"
else
	rm -f $(BIN)/webtabs $(APPS)/webtabs.desktop $(ICONS)/webtabs.png $(OLD_ICON) $(AUTOSTART)/webtabs.desktop
endif

# Release files in target/dist (Linux: tarball + .deb; macOS: the app, see build-macos.sh).
package:
ifeq ($(UNAME_S),Darwin)
	bash scripts/build-macos.sh
else
	bash scripts/package-linux.sh
endif

# Arch package from the current commit (target/dist/*.pkg.tar.zst).
package-arch:
	bash scripts/package-arch.sh
