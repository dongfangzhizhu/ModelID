#!/usr/bin/env bash
# One-shot frontend onboarding (C7): sync CAS into the virtual tree, then point
# the frontend models directory at it.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <comfyui|forge|a1111> <frontend-models-dir>" >&2
  exit 2
fi

frontend="$1"
target="$2"
store="${MODELD_STORE:-$HOME/.modeld}"

modeld --store "$store" frontend sync --frontend "$frontend"
modeld --store "$store" frontend connect "$frontend" --target "$target"
