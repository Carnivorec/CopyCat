#!/system/bin/sh

supervisors() {
    for d in /proc/[0-9]*; do

        case ${d#/proc/} in *[!0-9]*) continue ;; esac
        [ "$(tr '\0' ' ' 2>/dev/null < $d/cmdline)" = "sh /data/adb/copycat/copycatd.sh supervise " ] && echo ${d#/proc/}
    done
}

if [ -f /data/adb/copycat/copycatd.sh ]; then
    sh /data/adb/copycat/copycatd.sh stop > /dev/null 2>&1
fi
for p in $(supervisors); do kill $p 2>/dev/null; done
p=$(pidof copycatd) && kill $p
sleep 1
p=$(pidof copycatd) && kill -9 $p && sleep 1
if [ -n "$(pidof copycatd)" ]; then
    echo "守护进程无法停止，没有删除任何文件"
    exit 1
fi
if [ -n "$(supervisors)" ]; then
    echo "看门狗无法停止，没有删除任何文件"
    exit 1
fi

rm -f /data/adb/service.d/copycat.sh
rm -rf /data/local/copycat
rm -rf /data/adb/copycat

for f in /data/adb/service.d/copycat.sh /data/local/copycat /data/adb/copycat; do
    if [ -e $f ]; then
        echo "未能删除 $f"
        exit 1
    fi
done
echo ok
