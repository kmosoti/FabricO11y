# Disposable Debian 13 host with systemd as PID 1, for installation acceptance.
# PROXY and proxy-ca.crt are optional: they reach the package mirror through an
# HTTP(S) proxy and are removed before the image is committed.
FROM debian:trixie
ARG PROXY=
COPY proxy-ca.crt /tmp/proxy-ca.crt
RUN if [ -n "$PROXY" ]; then \
      printf 'Acquire::http::Proxy "%s";\nAcquire::https::Proxy "%s";\n' "$PROXY" "$PROXY" > /etc/apt/apt.conf.d/99proxy; \
      [ -s /tmp/proxy-ca.crt ] && echo 'Acquire::https::CaInfo "/tmp/proxy-ca.crt";' >> /etc/apt/apt.conf.d/99proxy; \
      sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources; \
    fi \
 && apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    systemd systemd-sysv dbus acl openssl ca-certificates procps curl python3 \
 && apt-get clean && rm -f /etc/apt/apt.conf.d/99proxy /tmp/proxy-ca.crt
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
