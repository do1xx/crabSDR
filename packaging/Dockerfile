# crabSDR – Laufzeit-Image (x86-64, ARM64, ARMv7). Das Binary wird vorher statisch gebaut (packaging/build-binaries.sh),
# hier kommt nur noch dazu, was zur Laufzeit nötig ist.
#   Voll:  docker buildx build -f packaging/Dockerfile --platform linux/amd64,linux/arm64,linux/arm/v7 -t crabsdr .
#   Lite (Alpine, nur RTL-SDR): siehe packaging/Dockerfile.lite
# rx_sdr (rx_tools) für SoapySDR-Geräte (Airspy, SDRplay über Fremdmodul, LimeSDR …) – nicht in Debian, daher selbst gebaut
FROM debian:bookworm-slim AS rxtools
ARG RX_TOOLS_COMMIT=811b21c4c8a592515279bd19f7460c6e4ff0551c
RUN apt-get update && apt-get install -y --no-install-recommends build-essential cmake git ca-certificates libsoapysdr-dev \
 && git clone https://github.com/rxseger/rx_tools /src && cd /src && git checkout $RX_TOOLS_COMMIT \
 && cmake -B build -DCMAKE_BUILD_TYPE=Release >/dev/null && cmake --build build -j4 && install -m 755 build/rx_sdr /rx_sdr

FROM debian:bookworm-slim
ARG TARGETARCH
RUN set -e; apt-get update; \
    apt-get install -y --no-install-recommends rtl-sdr hackrf soapysdr-tools \
      soapysdr-module-rtlsdr soapysdr-module-hackrf soapysdr-module-airspy soapysdr-module-mirisdr soapysdr-module-remote ca-certificates curl \
      direwolf multimon-ng wsjtx python3 python3-numpy python3-pil; \
    rm -rf /var/lib/apt/lists/*
COPY --from=rxtools /rx_sdr /usr/local/bin/rx_sdr
# Binary je Plattform: amd64, arm64, arm (v7)
COPY dist/bin/ /tmp/bin/
RUN A="$TARGETARCH"; [ "$A" = arm ] && A=armv7; install -m 755 /tmp/bin/$A/crabsdr-server /usr/local/bin/crabsdr-server && rm -rf /tmp/bin
COPY web/ /opt/crabsdr/web/
# Decoder-Plugins (decoder.json + Skript); eingeschaltet werden sie mit [[decoders]] in der config.toml
COPY plugins/aprs/ /opt/crabsdr/plugins/aprs/
COPY plugins/ft8/ /opt/crabsdr/plugins/ft8/
COPY plugins/sstv/ /opt/crabsdr/plugins/sstv/
COPY packaging/config.example.toml /opt/crabsdr/config.example.toml
COPY packaging/docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
COPY LICENSE /opt/crabsdr/LICENSE
RUN mkdir -p /data && chmod +x /usr/local/bin/docker-entrypoint.sh
ENV CRABSDR_CONFIG=/data/config.toml RUST_LOG=info
EXPOSE 8080
VOLUME /data
HEALTHCHECK --interval=60s --timeout=5s CMD ["curl", "-fsS", "-o", "/dev/null", "http://127.0.0.1:8080/bandinfo.js"]
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
