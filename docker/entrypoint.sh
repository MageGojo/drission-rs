#!/bin/sh
# 默认无头直接跑;需要「有头」(如易盾点选、部分强风控站点——无头不弹挑战)时设 USE_XVFB=1,
# 用 Xvfb 虚拟显示跑有头:服务器仍无物理显示器,但浏览器以为自己有真实显示。
#
# ⚠️ 为什么**不用** `xvfb-run`:它启动 Xvfb 后靠「Xvfb 就绪时给父进程发 SIGUSR1 + 父进程
#    wait/sigsuspend」来同步。当本脚本是容器 **PID 1** 时,PID 1 的信号语义特殊 + 该就绪信号
#    存在竞态,常导致父进程**永远卡在 sigsuspend**,真正的命令(drission-app)根本不被 exec
#    (现象:容器 Up 却无任何输出、没有业务进程、空转)。故这里改为**手动起 Xvfb + 轮询 X
#    socket 就绪**,再 exec 目标命令,彻底规避该 hang。
set -e

# 无头(默认):不需要 X,直接 exec。
if [ "${USE_XVFB:-0}" != "1" ]; then
  exec "$@"
fi

DISP_NUM="${DISPLAY_NUM:-99}"
SCREEN="${XVFB_SCREEN:-1280x1024x24}"
XSOCK="/tmp/.X11-unix/X${DISP_NUM}"

# 清理可能残留的锁/旧 socket(容器重启复用同一 :display 时)。
rm -f "/tmp/.X${DISP_NUM}-lock" "$XSOCK" 2>/dev/null || true

Xvfb ":${DISP_NUM}" -screen 0 "$SCREEN" -nolisten tcp >/tmp/xvfb.log 2>&1 &
XVFB_PID=$!

# 轮询 X socket 就绪(最多 ~10s),避免浏览器早于 X 起来而 "cannot open display"。
i=0
while [ ! -e "$XSOCK" ]; do
  i=$((i + 1))
  if [ "$i" -gt 100 ]; then
    echo "[entrypoint] Xvfb :$DISP_NUM 启动超时(10s);Xvfb 日志:" >&2
    cat /tmp/xvfb.log >&2 2>/dev/null || true
    exit 1
  fi
  # Xvfb 若已退出(端口冲突 / 缺字体路径等),提前报错,别空等。
  if ! kill -0 "$XVFB_PID" 2>/dev/null; then
    echo "[entrypoint] Xvfb 进程已退出;Xvfb 日志:" >&2
    cat /tmp/xvfb.log >&2 2>/dev/null || true
    exit 1
  fi
  sleep 0.1
done

export DISPLAY=":${DISP_NUM}"

# ── 窗口管理器 + 窗口激活(Xvfb「假有头」过强风控的关键「补环境」)────────────────────────
# 无 WM 时 Xvfb 里的 X11 窗口不被聚焦/激活 → Chrome 报 document.hasFocus()=false、页面被视为
# 非活动 → 易盾等行为风控**不下发点选挑战图**(里程碑 74 实测:补 WebGL 渲染器后仍不弹,根因即此)。
# fluxbox 是极轻量 WM,默认「给新窗口焦点」,Chrome 窗口一映射即被聚焦。
if command -v fluxbox >/dev/null 2>&1; then
  fluxbox >/tmp/fluxbox.log 2>&1 &
  sleep 0.5
  echo "[entrypoint] fluxbox 窗口管理器已启动(给浏览器窗口焦点)"
fi
# 兜底:后台持续把浏览器窗口激活并聚焦(xdotool),确保 hasFocus()=true。浏览器窗口在 app
# 启动后才映射(本脚本随后 exec 成 app),故先起一个独立后台循环,reparent 给 app 后照常运行;
# 跑约 180s(覆盖触发+换图重试窗口期)后自动退出,不长期占用。
if command -v xdotool >/dev/null 2>&1; then
  (
    n=0
    while [ "$n" -lt 180 ]; do
      wid="$(xdotool search --onlyvisible --class '[Cc]hrom' 2>/dev/null | head -n1)"
      if [ -n "$wid" ]; then
        xdotool windowactivate "$wid" >/dev/null 2>&1 || true
        xdotool windowfocus "$wid" >/dev/null 2>&1 || true
      fi
      n=$((n + 1))
      sleep 1
    done
  ) &
  echo "[entrypoint] xdotool 窗口激活守护已启动(兜底焦点)"
fi

echo "[entrypoint] Xvfb :$DISP_NUM 就绪(screen $SCREEN),DISPLAY=$DISPLAY → 启动:$*"
# exec:目标命令接管为容器主进程,直接拿到 stdio(日志直出),容器停止时连带收掉 Xvfb/WM。
exec "$@"
