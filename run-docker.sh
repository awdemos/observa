#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")"

sudo podman rm -f observa 2>/dev/null || true
sudo podman volume create observa-data 2>/dev/null || true

sudo podman build --cgroup-manager=cgroupfs --network=host --dns=1.1.1.1 --dns=8.8.8.8 -t observa:latest .

# Optional NVIDIA passthrough so GPU discovery works inside the container.
NVIDIA_ARGS=()
if [ -S /var/run/nvidia-persistenced/socket ] || [ -e /dev/nvidia0 ]; then
    NVIDIA_ARGS+=(--device /dev/nvidia0)
    for dev in /dev/nvidia*; do
        [ -e "$dev" ] && NVIDIA_ARGS+=(--device "$dev")
    done
fi
if command -v nvidia-smi >/dev/null 2>&1; then
    NVIDIA_ARGS+=(-v /usr/bin/nvidia-smi:/usr/bin/nvidia-smi:ro)
    for lib in /usr/lib64/libnvidia-ml.so* /usr/lib/x86_64-linux-gnu/libnvidia-ml.so*; do
        [ -e "$lib" ] || continue
        NVIDIA_ARGS+=(-v "$lib:/usr/lib/x86_64-linux-gnu/$(basename "$lib"):ro")
    done
fi
for pci_ids in /usr/share/hwdata/pci.ids /usr/share/misc/pci.ids; do
    if [ -f "$pci_ids" ]; then
        NVIDIA_ARGS+=(-v "$pci_ids:/usr/share/misc/pci.ids:ro")
        break
    fi
done

sudo podman run -d --name observa \
    --cgroup-manager=cgroupfs \
    --network=host \
    --cgroups=disabled \
    --dns=1.1.1.1 \
    --restart unless-stopped \
    -p 127.0.0.1:3000:3000 \
    -v observa-data:/data \
    ${OBSERVA_DASHBOARD_TOKEN:+-e OBSERVA_DASHBOARD_TOKEN="$OBSERVA_DASHBOARD_TOKEN"} \
    -e OBSERVA_BIND=0.0.0.0:3000 \
    -e OBSERVA_DATABASE_URL=sqlite:///data/observa.db \
    -e OBSERVA_LOG_SOURCE=file \
    -e OBSERVA_LOG_FILE=/data/logs/system.log \
    -e RUST_LOG=info \
    "${NVIDIA_ARGS[@]}" \
    observa:latest

if [ -n "${OBSERVA_DASHBOARD_TOKEN:-}" ]; then
    echo "Dashboard token: $OBSERVA_DASHBOARD_TOKEN"
    echo "Open http://127.0.0.1:3000 and log in with the token above."
else
    echo "Open http://127.0.0.1:3000 (no dashboard token set; running open on localhost)."
fi
echo "Logs: sudo podman logs -f observa"
