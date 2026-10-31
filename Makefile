PREFIX ?= $(HOME)/.local
BIN     = $(PREFIX)/bin
APPS    = $(PREFIX)/share/applications
ICONS   = $(PREFIX)/share/icons/hicolor/scalable/apps
AUTOSTART = $(HOME)/.config/autostart
CARGO = bash scripts/cargo.sh
UNAME_S := $(shell uname -s)

.PHONY: build test install uninstall autostart run

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
	install -Dm644 assets/webtabs.svg $(ICONS)/webtabs.svg
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
	rm -f $(BIN)/webtabs $(APPS)/webtabs.desktop $(ICONS)/webtabs.svg $(AUTOSTART)/webtabs.desktop
endif
