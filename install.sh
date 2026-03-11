#!/bin/bash
set -e

echo "Installing GitAnalyser..."

# Check if running as root or via sudo
if [ "$EUID" -ne 0 ]; then
  echo "Please run this script with sudo to install into system directories."
  echo "Example: sudo ./install.sh"
  exit 1
fi

# Paths
BIN_DIR="/usr/local/bin"
MAN_DIR="/usr/local/share/man/man1"

# Create directories if they don't exist
mkdir -p "$BIN_DIR"
mkdir -p "$MAN_DIR"

# Install binary
if [ -f "GitAnalyser" ]; then
  echo "Installing binary to $BIN_DIR/GitAnalyser"
  cp "GitAnalyser" "$BIN_DIR/"
  chmod +x "$BIN_DIR/GitAnalyser"
else
  echo "Error: GitAnalyser binary not found in current directory."
  exit 1
fi

# Install man page
if [ -f "GitAnalyser.1" ]; then
  echo "Installing man page to $MAN_DIR/GitAnalyser.1"
  cp "GitAnalyser.1" "$MAN_DIR/"
  chmod 644 "$MAN_DIR/GitAnalyser.1"

  # Update man db if available
  if command -v mandb >/dev/null 2>&1; then
    echo "Updating man database..."
    mandb -q >/dev/null 2>&1 || true
  fi
else
  echo "Warning: GitAnalyser.1 man page not found, skipping man page installation."
fi

echo ""
echo "Installation complete!"
echo "You can now run 'GitAnalyser' from anywhere."
echo "Type 'man GitAnalyser' or 'GitAnalyser --help' for usage instructions."
