#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# xsynth PipeWire 低延迟音频一键迁移脚本
# 仅在 Linux 上执行，其他平台不受影响
# ============================================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log()  { echo -e "${GREEN}[✓]${NC} $1"; }
warn() { echo -e "${YELLOW}[!]${NC} $1"; }
err()  { echo -e "${RED}[✗]${NC} $1"; }

# 检查是否为 Linux
if [[ "$(uname)" != "Linux" ]]; then
    err "此脚本仅在 Linux 上执行"
    exit 1
fi

# 检测包管理器
if command -v apt-get &>/dev/null; then
    PKG="apt-get install -y"
elif command -v dnf &>/dev/null; then
    PKG="dnf install -y"
elif command -v pacman &>/dev/null; then
    PKG="pacman -S --noconfirm"
else
    err "不支持的包管理器，请手动安装 PipeWire"
    exit 1
fi

echo "=============================================="
echo " xsynth PipeWire 低延迟音频迁移"
echo "=============================================="

# ---- Step 1: 安装 PipeWire ----
echo ""
echo "Step 1/4: 安装 PipeWire 及组件"

if command -v apt-get &>/dev/null; then
    sudo apt-get update -qq
    sudo $PKG pipewire pipewire-pulse pipewire-jack pipewire-alsa wireplumber
    sudo $PKG alsa-utils qpwgraph helvum
elif command -v dnf &>/dev/null; then
    sudo $PKG pipewire pipewire-pulseaudio pipewire-jack pipewire-alsa wireplumber
    sudo $PKG alsa-utils qpwgraph helvum
elif command -v pacman &>/dev/null; then
    sudo $PKG pipewire pipewire-pulse pipewire-jack pipewire-alsa wireplumber
    sudo $PKG alsa-utils qpwgraph helvum
fi
log "PipeWire 安装完成"

# ---- Step 2: 低延迟配置 ----
echo ""
echo "Step 2/4: 配置低延迟参数"

sudo mkdir -p /etc/pipewire/pipewire.conf.d

# PipeWire 时钟和量子配置
sudo tee /etc/pipewire/pipewire.conf.d/10-low-latency.conf > /dev/null << 'CONF'
context.properties = {
    default.clock.rate          = 48000
    default.clock.allowed-rates = [ 44100, 48000, 96000, 192000 ]
    default.clock.quantum       = 256
    default.clock.min-quantum   = 32
    default.clock.max-quantum   = 8192
    default.clock.quantum-limit = 8192
    default.clock.quantum-floor = 4
    clock.power-of-two-quantum  = true
}
CONF

# 实时优先级配置
sudo tee /etc/pipewire/pipewire.conf.d/20-rt-config.conf > /dev/null << 'CONF'
context.modules = [
    { name = libpipewire-module-rt
        args = {
            nice.level   = -19
            rt.prio      = 88
            rt.time.soft = 200000
            rt.time.hard = 200000
        }
        flags = [ ifexists nofail ]
    }
]
CONF

log "PipeWire 低延迟参数配置完成"

# ---- Step 3: WirePlumber ALSA 缓冲优化 ----
echo ""
echo "Step 3/4: 配置 ALSA 缓冲优化"

sudo mkdir -p /etc/wireplumber/main.lua.d
sudo tee /etc/wireplumber/main.lua.d/51-alsa-buffer.lua > /dev/null << 'LUA'
alsa_monitor.rules = {
    {
        matches = {
            {
                { "node.name", "matches", "alsa_output.*" },
            },
        },
        apply_properties = {
            ["api.alsa.period-size"]   = 256,
            ["api.alsa.period-num"]    = 2,
            ["api.alsa.headroom"]      = 0,
            ["api.alsa.start-delay"]   = 0,
            ["audio.rate"]             = 48000,
            ["resample.quality"]       = 4,
            ["session.suspend-timeout-seconds"] = 0,
        },
    },
    {
        matches = {
            {
                { "node.name", "matches", "alsa_input.*" },
            },
        },
        apply_properties = {
            ["api.alsa.period-size"]   = 256,
            ["api.alsa.period-num"]    = 2,
            ["audio.rate"]             = 48000,
            ["session.suspend-timeout-seconds"] = 0,
        },
    },
}
LUA

log "WirePlumber ALSA 缓冲优化完成"

# ---- Step 4: 实时权限 ----
echo ""
echo "Step 4/4: 配置实时音频权限"

sudo tee /etc/security/limits.d/90-audio.conf > /dev/null << 'LIMITS'
@audio   -  rtprio     95
@audio   -  memlock    unlimited
@audio   -  nice      -19
LIMITS

# 将当前用户加入 audio 组
if [[ "$SUDO_USER" ]]; then
    sudo usermod -a -G audio "$SUDO_USER"
    log "用户 $SUDO_USER 已加入 audio 组（需重新登录生效）"
else
    warn "未检测到 sudo 调用者，请手动将你的用户加入 audio 组："
    warn "  sudo usermod -a -G audio \$USER"
fi

# ---- 启用服务 ----
echo ""
echo "启动 PipeWire 服务"

systemctl --user enable --now pipewire pipewire-pulse wireplumber 2>/dev/null || {
    warn "无法以 systemd user 模式启动服务"
    warn "请手动执行: systemctl --user enable --now pipewire pipewire-pulse wireplumber"
}

echo ""
echo "=============================================="
echo -e "${GREEN} PipeWire 迁移完成！${NC}"
echo "=============================================="
echo ""
echo "下一步："
echo "  1. 重新登录（使 audio 组权限生效）"
echo "  2. 验证音频: pw-cli info | head -5"
echo "  3. 编译 xsynth 时启用 PipeWire 后端:"
echo "     cargo build --features pipewire"
echo "  4. 运行 xsynth 实时合成器时自动使用 PipeWire"
echo "  5. 用 qpwgraph 可视化音频路由"
echo ""