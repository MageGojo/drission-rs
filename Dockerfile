# drission-rs 服务器一键镜像 —— 让本库在「没有桌面 / 没有显示器(无 UI)」的 Linux 服务器上
# 直接跑无头浏览器自动化。镜像里装好 Chrome 及其**全部系统 .so 依赖 + 中日韩字体**,
# 你的程序则以无头模式驱动它;库已在 Linux 自动补 `--no-sandbox` / `--disable-dev-shm-usage`,
# 故在 root / 容器内也不会崩。`docker run` 即用,目标服务器无需装任何东西。
#
# ── 构建(默认编 cdp_demo 示例)──
#   docker build -t drission .
# ── 换成你自己的示例 / feature 组合 ──
#   docker build --build-arg EXAMPLE=cdp_fetch --build-arg FEATURES=cdp,ocr -t drission .
# ── 运行(/dev/shm 调大更稳;库已自动 --disable-dev-shm-usage 兜底,不调也能跑)──
#   docker run --rm --shm-size=1g drission
# ── 出 x86_64 + ARM64 两份(覆盖鲲鹏/倚天/飞腾等信创 ARM 服务器)──
#   docker buildx build --platform linux/amd64,linux/arm64 -t <you>/drission --push .
#
# 把本库用进**你自己的项目**:把第二阶段当基础镜像(已带 Chrome+字体+CHROME_BIN),
# 只需 COPY 进你自己的二进制并改 ENTRYPOINT 即可。

# ── Stage 1:编译 ──────────────────────────────────────────────────────────────
# 容器内环境固定,用默认 gnu 目标即可(musl 静态是给「裸机 scp 二进制」用的,容器里不需要)。
FROM rust:1-bookworm AS builder
# aws-lc-sys(reqwest 的 rustls C 依赖)需 cmake + clang;其余 C 依赖用镜像自带 gcc。
RUN apt-get update && apt-get install -y --no-install-recommends \
        cmake clang libclang-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY . .
ARG EXAMPLE=cdp_demo
ARG FEATURES=cdp
# 兼容 legacy builder 与 BuildKit(不用 BuildKit 专属的 --mount=cache,确保 `docker build` 到处都能跑)。
RUN cargo build --release --features "${FEATURES}" --example "${EXAMPLE}" \
    && cp "target/release/examples/${EXAMPLE}" /build/app

# ── Stage 2:运行 ──────────────────────────────────────────────────────────────
# Debian slim + Chrome + 字体,体积最小。
FROM debian:bookworm-slim AS runtime
ENV DEBIAN_FRONTEND=noninteractive
# amd64 装**真·Google Chrome**(过盾最佳,且自动拉齐所有 .so 依赖);
# 其它架构(arm64 等)无官方 Linux 版 Chrome,故装发行版 chromium(同样自动带依赖)。
# 用 `dpkg --print-architecture` 判定本镜像真实架构(buildx 多架构与普通 build 都准)。
# 两者都补**中日韩字体**(否则中文站全是 ▯ 方块)+ emoji 字体。
# xvfb + xauth:支持「有头」跑在虚拟显示里(USE_XVFB=1,见 entrypoint.sh);无头默认用不到、开销也小。
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates fonts-liberation fonts-noto-cjk fonts-noto-color-emoji xvfb xauth \
    && arch="$(dpkg --print-architecture)" \
    && if [ "$arch" = "amd64" ]; then \
         apt-get install -y --no-install-recommends wget gnupg \
         && wget -qO- https://dl.google.com/linux/linux_signing_key.pub \
              | gpg --dearmor -o /usr/share/keyrings/google-chrome.gpg \
         && echo "deb [arch=amd64 signed-by=/usr/share/keyrings/google-chrome.gpg] http://dl.google.com/linux/chrome/deb/ stable main" \
              > /etc/apt/sources.list.d/google-chrome.list \
         && apt-get update && apt-get install -y --no-install-recommends google-chrome-stable \
         && ln -sf /usr/bin/google-chrome-stable /usr/local/bin/chrome \
         && apt-get purge -y wget gnupg ; \
       else \
         apt-get install -y --no-install-recommends chromium \
         && ln -sf /usr/bin/chromium /usr/local/bin/chrome ; \
       fi \
    && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*
# fluxbox + xdotool:Xvfb「假有头」过强风控(易盾点选等)的关键「补环境」——无窗口管理器时 X11
#   窗口不被聚焦,Chrome 的 document.hasFocus()=false、窗口不激活,行为风控据此判定非真人桌面而
#   **不下发挑战图**(里程碑 73/74 实测:补 WebGL 渲染器后仍不弹,根因即缺窗口焦点)。fluxbox 给新
#   窗口焦点,xdotool 兜底强制激活(见 entrypoint.sh)。二者仅在 USE_XVFB=1 的有头路径用到、无头零开销;
#   单独成层(不并入上面的 Chrome 安装层),以免每次微调都重装 Chrome。
RUN apt-get update && apt-get install -y --no-install-recommends fluxbox xdotool \
    && apt-get autoremove -y && rm -rf /var/lib/apt/lists/*
# 库定位浏览器时 CHROME_BIN 优先级最高(见 src/cdp/locate.rs),指向上面装好的 Chrome。
ENV CHROME_BIN=/usr/local/bin/chrome
COPY --from=builder /build/app /usr/local/bin/drission-app
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
# 非 root 运行(纵深防御)。库在 Linux 已自动加 --no-sandbox,故非 root 同样不崩。
RUN chmod +x /usr/local/bin/entrypoint.sh /usr/local/bin/drission-app \
    && useradd -m -u 10001 app
USER app
WORKDIR /home/app
# entrypoint 默认无头直跑;`-e USE_XVFB=1` 则用 Xvfb 虚拟显示跑有头(易盾点选等无头不弹挑战的场景)。
ENTRYPOINT ["/usr/local/bin/entrypoint.sh", "/usr/local/bin/drission-app"]
