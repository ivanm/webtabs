PREFIX ?= $(HOME)/.local
BIN     = $(PREFIX)/bin
APPS    = $(PREFIX)/share/applications
ICONS   = $(PREFIX)/share/icons/hicolor/scalable/apps
AUTOSTART = $(HOME)/.config/autostart

.PHONY: build test install uninstall autostart run

build:
	cargo build --release

test:
	cargo test
	cargo clippy --all-targets -- -D warnings

run:
	cargo run

install: build
	install -Dm755 target/release/webtabs $(BIN)/webtabs
	install -Dm644 packaging/webtabs.desktop $(APPS)/webtabs.desktop
	install -Dm644 assets/webtabs.svg $(ICONS)/webtabs.svg
	-update-desktop-database $(APPS) 2>/dev/null
	-gtk-update-icon-cache -q $(PREFIX)/share/icons/hicolor 2>/dev/null

# Start hidden in the tray on login.
autostart: install
	mkdir -p $(AUTOSTART)
	sed 's|^Exec=webtabs|Exec=$(BIN)/webtabs --hidden|' packaging/webtabs.desktop > $(AUTOSTART)/webtabs.desktop

uninstall:
	rm -f $(BIN)/webtabs $(APPS)/webtabs.desktop $(ICONS)/webtabs.svg $(AUTOSTART)/webtabs.desktop
