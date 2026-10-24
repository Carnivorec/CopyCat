#!/system/bin/sh

PKG=app.copycat
ROOT=/data/adb/copycat
SELF=$ROOT/copycatd.sh
DATA=/data/local/copycat
KEY=$ROOT/master.key
SUPERVISOR=$ROOT/supervisor.pid

WAKE=$ROOT/wake
STOPPED=$ROOT/stopped

APK_USED=$ROOT/apk
BOOT=/data/adb/service.d/copycat.sh
LOG=$DATA/daemon.log

daemon_pid() { pidof copycatd; }

supervisor_alive() {
    [ -f $SUPERVISOR ] || return 1
    tr '\0' ' ' 2>/dev/null < "/proc/$(cat $SUPERVISOR)/cmdline" | grep -q "$SELF supervise"
}

prepare() {
    umask 077
    mkdir -p $ROOT $DATA
    chmod 700 $ROOT
    chown 2000:2000 $DATA
    chmod 700 $DATA
    if [ ! -s $KEY ]; then
        head -c 32 /dev/urandom > $KEY.tmp && mv -f $KEY.tmp $KEY
    fi
    chmod 600 $KEY
}

log() { echo "$(date '+%m-%d %H:%M:%S') [看门狗] $*" >> $LOG; }

run_once() {
    APK=$(pm path $PKG 2>/dev/null | head -n 1 | cut -d: -f2)
    if [ -z "$APK" ]; then
        log "未找到 CopyCat App"
        return 1
    fi
    case "$(getprop ro.product.cpu.abi)" in
        x86_64) LIB=$(dirname "$APK")/lib/x86_64 ;;
        *) LIB=$(dirname "$APK")/lib/arm64 ;;
    esac

    APP_UID=$(stat -c %u /data/user_de/0/$PKG)
    echo "$APK" > $APK_USED
    if [ -f $LOG ] && [ "$(stat -c %s $LOG)" -gt 1048576 ]; then
        mv -f $LOG $LOG.1
    fi

    CLASSPATH="$APK" "$LIB/libcopycatd.so" app_process /system/bin --nice-name=copycatd         app.copycat.daemon.Daemon "$DATA" "$LIB" "$APP_UID" < $KEY >> $LOG 2>&1
}

supervise() {
    echo $$ > $SUPERVISOR
    rm -f $WAKE
    fails=0
    while [ ! -f $STOPPED ]; do
        t0=$(date +%s)
        run_once &
        child=$!
        sleep 3
        p=$(daemon_pid)
        [ -n "$p" ] && echo -800 > /proc/$p/oom_score_adj
        wait $child
        [ -f $STOPPED ] && break
        if [ $(( $(date +%s) - t0 )) -lt 60 ]; then fails=$((fails + 1)); else fails=0; fi
        [ $fails -gt 6 ] && fails=6
        delay=$((5 << fails))
        [ $delay -gt 300 ] && delay=300
        log "守护进程退出，${delay} 秒后重启"
        waited=0
        while [ $waited -lt $delay ] && [ ! -f $WAKE ] && [ ! -f $STOPPED ]; do
            sleep 5
            waited=$((waited + 5))
        done
        if [ -f $WAKE ]; then
            rm -f $WAKE
            fails=0
            log "收到启动请求，立即重启"
        fi
    done
    rm -f $SUPERVISOR
}

case "$1" in
start)
    prepare
    rm -f $STOPPED
    if supervisor_alive; then
        touch $DATA/attach
        [ -z "$(daemon_pid)" ] && touch $WAKE
    else

        p=$(daemon_pid)
        [ -n "$p" ] && kill $p && sleep 1
        setsid sh $SELF supervise < /dev/null > /dev/null 2>&1 &
    fi
    echo ok
    ;;
supervise)
    supervise
    ;;
stop)
    touch $STOPPED
    supervisor_alive && kill "$(cat $SUPERVISOR)"
    p=$(daemon_pid)
    [ -n "$p" ] && kill $p
    rm -f $SUPERVISOR
    echo ok
    ;;
status)
    p=$(daemon_pid)
    if [ -n "$p" ]; then echo "running $p"; else echo stopped; fi
    ;;
attach)
    p=$(daemon_pid)
    apk=$(pm path $PKG 2>/dev/null | head -n 1 | cut -d: -f2)
    if [ -n "$p" ] && [ -n "$apk" ] && [ "$(cat $APK_USED 2>/dev/null)" != "$apk" ]; then

        log "App 已更新，使用新版本重启守护进程"
        sh $SELF stop > /dev/null
        sh $SELF start
    elif [ -n "$p" ]; then
        touch $DATA/attach
        echo ok
    else
        sh $SELF start
    fi
    ;;
boot-on)
    mkdir -p /data/adb/service.d
    cat > $BOOT <<EOF
#!/system/bin/sh
# CopyCat 开机自启（删除本文件即可取消）
(until [ "\$(getprop sys.boot_completed)" = 1 ]; do sleep 3; done; sh $SELF start) &
EOF
    chmod 755 $BOOT
    echo ok
    ;;
boot-off)
    rm -f $BOOT
    echo ok
    ;;
boot-status)
    if [ -f $BOOT ]; then echo on; else echo off; fi
    ;;
*)
    echo "用法: $0 start|stop|status|attach|boot-on|boot-off|boot-status"
    exit 2
    ;;
esac
