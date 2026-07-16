# DragonFly BSD service

This directory contains the native `rc.d` service asset for DragonFly BSD
6.4+. It uses DragonFly's `daemon(8)` supervisor because `pcloudd serve` is a
foreground process and does not create a service PID file itself.

Create the service identity and protected state/configuration directories:

```sh
pw groupadd pcloudd
pw useradd pcloudd -g pcloudd -c "pcloud-rs daemon" \
  -d /var/lib/pcloud-rs -s /usr/sbin/nologin -w no
install -d -o pcloudd -g pcloudd -m 0700 /var/lib/pcloud-rs
install -d -o root -g wheel -m 0755 /usr/local/etc/pcloud-rs
install -d -o root -g wheel -m 0755 /usr/local/libexec
install -m 0555 packaging/init/common/pcloudd-wrapper.sh \
  /usr/local/libexec/pcloudd-wrapper.sh
install -m 0600 packaging/init/common/pcloudd.env.example \
  /usr/local/etc/pcloud-rs/pcloudd.env
install -m 0555 packaging/dragonfly/pcloudd \
  /usr/local/etc/rc.d/pcloudd
```

Put a token in the configured owner-only credential file, then enable and
exercise the service:

```sh
sysrc pcloudd_enable=YES
service pcloudd start
service pcloudd status
pcloudc status
service pcloudd stop
```

The script records the supervisor in `/var/run/pcloudd.pid`. DragonFly's
`daemon(8)` forwards service shutdown to the child and removes the locked PID
file when the process exits. Installation and upgrade testing on a retained
native release job is still required before advertising a supported package.
