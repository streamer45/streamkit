#!/usr/bin/env bash
# SPDX-FileCopyrightText: © 2025 StreamKit Contributors
#
# SPDX-License-Identifier: MPL-2.0

set -euo pipefail
IFS=$'\n\t'

REPO_DEFAULT="streamer45/streamkit"
TAG=""
REPO="$REPO_DEFAULT"
INSTALL_PREFIX="/opt/streamkit"
NO_START="0"
UNINSTALL="0"
PURGE="0"
YES="0"

usage() {
	cat <<'EOF'
Usage: install.sh [OPTIONS]

Installs StreamKit from GitHub Releases and registers a systemd service.

Options:
  --tag vX.Y.Z        Install a specific release tag (required unless --latest)
  --latest            Install the latest GitHub Release
  --repo owner/name   GitHub repo to install from (default: streamer45/streamkit)
  --prefix PATH       Install prefix (default: /opt/streamkit)
  --no-start          Only install files; don't enable/start the service
  --uninstall         Uninstall StreamKit (removes service and binaries)
  --purge             With --uninstall: also remove config, data, and user
  -y, --yes           Skip confirmation prompts
  -h, --help          Show help

Examples:
  sudo ./install.sh --tag v0.2.0
  sudo ./install.sh --latest
  sudo ./install.sh --uninstall
  sudo ./install.sh --uninstall --purge
EOF
}

need_cmd() {
	command -v "$1" >/dev/null 2>&1 || {
		echo "Missing required command: $1" >&2
		exit 1
	}
}

while [[ $# -gt 0 ]]; do
	case "$1" in
		--tag|--version)
			TAG="${2:-}"
			shift 2
			;;
		--latest)
			TAG="__latest__"
			shift
			;;
		--repo)
			REPO="${2:-}"
			shift 2
			;;
		--prefix)
			INSTALL_PREFIX="${2:-}"
			shift 2
			;;
		--no-start)
			NO_START="1"
			shift
			;;
		--uninstall)
			UNINSTALL="1"
			shift
			;;
		--purge)
			PURGE="1"
			shift
			;;
		-y|--yes)
			YES="1"
			shift
			;;
		-h|--help)
			usage
			exit 0
			;;
		*)
			echo "Unknown argument: $1" >&2
			usage >&2
			exit 2
			;;
	esac
done

if [[ "${EUID:-$(id -u)}" -ne 0 ]]; then
	echo "Run as root (e.g. via sudo)." >&2
	exit 1
fi

do_uninstall() {
	echo "This will remove:"
	echo "  - StreamKit binaries (${INSTALL_PREFIX})"
	echo "  - systemd service unit"
	if [[ "$PURGE" == "1" ]]; then
		echo "  - Configuration (/etc/streamkit)"
		echo "  - Data and plugins (/var/lib/streamkit)"
		echo "  - streamkit system user"
	fi
	echo ""
	if [[ "$YES" != "1" ]]; then
		read -r -p "Continue? [y/N] " confirm
		if [[ ! "$confirm" =~ ^[Yy]$ ]]; then
			echo "Aborted."
			exit 0
		fi
	fi

	echo ""
	echo "Uninstalling StreamKit..."

	if systemctl is-active --quiet streamkit 2>/dev/null; then
		echo "Stopping streamkit service..."
		systemctl stop streamkit
	fi

	if systemctl is-enabled --quiet streamkit 2>/dev/null; then
		echo "Disabling streamkit service..."
		systemctl disable streamkit
	fi

	if [[ -f /etc/systemd/system/streamkit.service ]]; then
		echo "Removing service unit..."
		rm -f /etc/systemd/system/streamkit.service
		systemctl daemon-reload
	fi

	if [[ -d "$INSTALL_PREFIX" ]]; then
		echo "Removing installation directory ${INSTALL_PREFIX}..."
		rm -rf "$INSTALL_PREFIX"
	fi

	if [[ "$PURGE" == "1" ]]; then
		echo "Purging configuration and data..."

		if [[ -d /etc/streamkit ]]; then
			echo "Removing /etc/streamkit..."
			rm -rf /etc/streamkit
		fi

		if [[ -d /var/lib/streamkit ]]; then
			echo "Removing /var/lib/streamkit..."
			rm -rf /var/lib/streamkit
		fi

		if id -u streamkit >/dev/null 2>&1; then
			echo "Removing streamkit user..."
			userdel streamkit 2>/dev/null || true
		fi

		if getent group streamkit >/dev/null 2>&1; then
			echo "Removing streamkit group..."
			groupdel streamkit 2>/dev/null || true
		fi
	else
		echo ""
		echo "Note: Config (/etc/streamkit) and data (/var/lib/streamkit) preserved."
		echo "Use --purge to remove everything including the streamkit user."
	fi

	echo "StreamKit uninstalled."
	exit 0
}

if [[ "$UNINSTALL" == "1" ]]; then
	do_uninstall
fi

need_cmd curl
need_cmd tar
need_cmd sed
need_cmd sha256sum
need_cmd systemctl
need_cmd install

arch="$(uname -m)"
case "$arch" in
	x86_64|amd64) platform="linux-x64" ;;
	*)
		echo "Unsupported architecture: ${arch}" >&2
		exit 1
		;;
esac

gh_api_latest_tag() {
	need_cmd sed
	need_cmd head
	curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
		| sed -n 's/^[[:space:]]*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p' \
		| head -n 1
}

if [[ -z "$TAG" ]]; then
	echo "Missing --tag (or use --latest)." >&2
	exit 2
fi

if [[ "$TAG" == "__latest__" ]]; then
	TAG="$(gh_api_latest_tag)"
	if [[ -z "$TAG" ]]; then
		echo "Failed to resolve latest release tag for ${REPO}." >&2
		exit 1
	fi
fi

asset="streamkit-${TAG}-${platform}.tar.gz"
base_url="https://github.com/${REPO}/releases/download/${TAG}"
tmp_dir="$(mktemp -d)"
cleanup() { rm -rf "$tmp_dir"; }
trap cleanup EXIT

echo "Downloading ${asset} from ${REPO} (${TAG})..."
curl -fsSL -o "${tmp_dir}/${asset}" "${base_url}/${asset}"
curl -fsSL -o "${tmp_dir}/${asset}.sha256" "${base_url}/${asset}.sha256"

(
	cd "$tmp_dir"
	sha256sum -c "${asset}.sha256"
)

echo "Extracting..."
tar -xzf "${tmp_dir}/${asset}" -C "$tmp_dir"
bundle_dir="${tmp_dir}/streamkit-${TAG}"
if [[ ! -d "$bundle_dir" ]]; then
	echo "Unexpected tarball contents (missing $(basename "$bundle_dir")/ directory)." >&2
	exit 1
fi

mkdir -p "${INSTALL_PREFIX}/releases"
mkdir -p "${INSTALL_PREFIX}"
release_dir="${INSTALL_PREFIX}/releases/${TAG}"
if [[ -e "$release_dir" ]]; then
	echo "Release already installed at ${release_dir}."
else
	mv "$bundle_dir" "$release_dir"
fi

ln -sfn "$release_dir" "${INSTALL_PREFIX}/current"
ln -sfn "current/skit" "${INSTALL_PREFIX}/skit"
ln -sfn "current/skit-cli" "${INSTALL_PREFIX}/skit-cli"

if ! id -u streamkit >/dev/null 2>&1; then
	useradd --system --home-dir /var/lib/streamkit --create-home --shell /usr/sbin/nologin streamkit
fi

install -d -m 0755 /etc/streamkit
install -d -m 0755 /var/lib/streamkit/plugins
chown -R streamkit:streamkit /var/lib/streamkit

script_dir="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

install_unit() {
	local dst="/etc/systemd/system/streamkit.service"
	if [[ -f "${script_dir}/streamkit.service" ]]; then
		sed "s|/opt/streamkit|${INSTALL_PREFIX}|g" "${script_dir}/streamkit.service" >"$dst"
		chmod 0644 "$dst"
		return
	fi
	cat >"$dst" <<EOF
[Unit]
Description=StreamKit Server
Documentation=https://streamkit.dev
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=streamkit
Group=streamkit
EnvironmentFile=-/etc/streamkit/streamkit.env
ExecStart=${INSTALL_PREFIX}/skit --config /etc/streamkit/skit.toml serve
WorkingDirectory=${INSTALL_PREFIX}
Restart=always
RestartSec=5
NoNewPrivileges=true
PrivateTmp=true
StateDirectory=streamkit
LogsDirectory=streamkit
StandardOutput=journal
StandardError=journal
SyslogIdentifier=streamkit

[Install]
WantedBy=multi-user.target
EOF
}

install_config() {
	local dst="/etc/streamkit/skit.toml"
	if [[ -f "$dst" ]]; then
		return
	fi
	if [[ -f "${script_dir}/skit.toml" ]]; then
		install -m 0644 "${script_dir}/skit.toml" "$dst"
		return
	fi
	cat >"$dst" <<'EOF'
[server]
address = "127.0.0.1:4545"

[plugins]
directory = "/var/lib/streamkit/plugins"

[log]
console_enable = true
file_enable = false
console_level = "info"
EOF
	chmod 0644 "$dst"
}

install_env() {
	local dst="/etc/streamkit/streamkit.env"
	if [[ -f "$dst" ]]; then
		return
	fi
	if [[ -f "${script_dir}/streamkit.env" ]]; then
		install -m 0644 "${script_dir}/streamkit.env" "$dst"
		return
	fi
	cat >"$dst" <<'EOF'
RUST_LOG=info
EOF
	chmod 0644 "$dst"
}

install_unit
install_config
install_env

systemctl daemon-reload

if [[ "$NO_START" == "1" ]]; then
	echo "Installed. Start with: systemctl enable --now streamkit"
	exit 0
fi

systemctl enable --now streamkit
systemctl status --no-pager streamkit || true
