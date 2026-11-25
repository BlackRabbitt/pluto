#!/bin/sh
set -eu

apk add --no-progress sudo ca-certificates
id pluto-api >/dev/null 2>&1 || adduser -D -H -s /sbin/nologin -h /var/lib/pluto pluto-api
id pluto-tool >/dev/null 2>&1 || adduser -D -H -s /sbin/nologin -h /var/lib/pluto-workspace pluto-tool

install -d -m 0700 -o pluto-api -g pluto-api /var/lib/pluto
install -d -m 0755 -o pluto-tool -g pluto-tool /var/lib/pluto-workspace
cat > pluto.sudoers <<'RULES'
Defaults:pluto-api !use_pty
pluto-api ALL=(pluto-tool) NOPASSWD: NOSETENV: /usr/local/bin/pluto --vm-run-tool *
RULES
visudo -cf pluto.sudoers

if [ -x /etc/init.d/pluto-machine ]; then
    rc-service -s pluto-machine stop
    if [ -f /etc/conf.d/pluto-machine ] && [ ! -e /etc/conf.d/pluto ] && [ ! -L /etc/conf.d/pluto ]; then
        cp -p /etc/conf.d/pluto-machine /etc/conf.d/pluto
    fi
    if [ -L /etc/runlevels/default/pluto-machine ]; then
        rc-update del pluto-machine default
    fi
fi
if [ -x /etc/init.d/pluto ]; then
    rc-service -s pluto stop
fi
# Replace only the old installer-managed grant so upgrades preserve other rules.
for policy in /etc/sudoers.d/pluto /etc/sudoers.d/pluto-machine; do
    [ -f "$policy" ] || continue
    sed '/^pluto-api ALL=(pluto-tool) NOPASSWD: \/usr\/bin\/env -i \*$/d' "$policy" > pluto-migrated.sudoers
    if ! cmp -s "$policy" pluto-migrated.sudoers; then
        visudo -cf pluto-migrated.sudoers
        install -m 0440 pluto-migrated.sudoers "$policy"
    fi
done
install -m 0440 pluto.sudoers /etc/sudoers.d/pluto-tool
install -m 0755 pluto /usr/local/bin/pluto
install -m 0755 pluto.init /etc/init.d/pluto
rc-update add pluto default
rc-service pluto start
if [ -x /etc/init.d/pluto-machine ]; then
    rm /etc/init.d/pluto-machine
fi
# The next boot can reuse the cached dependency tree from before the rename.
rc-update -u
