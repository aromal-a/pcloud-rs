# illumos and Oracle Solaris service

`pcloudd.xml` defines a disabled-by-default SMF child service. SMF runs the
foreground daemon as the dedicated `pcloudd` identity, restarts it according
to the child-service model, sends SIGTERM on stop, and sends SIGHUP on refresh.

The service files use the standard third-party SMF locations:

- manifest: `/lib/svc/manifest/site/pcloud-rs.xml`
- method: `/lib/svc/method/pcloudd`

Provision the identity and state before importing the manifest:

```sh
groupadd pcloudd
useradd -g pcloudd -d /var/lib/pcloud-rs -s /usr/bin/false \
  -c "pcloud-rs daemon" pcloudd
install -d -o pcloudd -g pcloudd -m 0700 /var/lib/pcloud-rs
install -d -o root -g root -m 0755 /etc/pcloud-rs /usr/local/libexec
install -m 0555 packaging/init/common/pcloudd-wrapper.sh \
  /usr/local/libexec/pcloudd-wrapper.sh
install -m 0440 packaging/init/common/pcloudd.env.example \
  /etc/pcloud-rs/pcloudd.env
chown root:pcloudd /etc/pcloud-rs/pcloudd.env
install -m 0555 packaging/solarish/pcloudd /lib/svc/method/pcloudd
install -m 0444 packaging/solarish/pcloudd.xml \
  /lib/svc/manifest/site/pcloud-rs.xml
svccfg validate /lib/svc/manifest/site/pcloud-rs.xml
svcadm restart svc:/system/manifest-import:default
svcadm enable svc:/site/pcloud-rs:default
```

Inspect failures with `svcs -xv svc:/site/pcloud-rs:default` and the service
log shown by `svcs -L svc:/site/pcloud-rs:default`. The portable tar candidate
is not an IPS repository; native installation and upgrade evidence remains a
release qualification requirement.
