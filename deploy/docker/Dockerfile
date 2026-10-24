FROM scratch
ARG TARGETARCH
COPY --chmod=0755 bin/${TARGETARCH}/copycat-relay /copycat-relay
USER 65532:65532
ENV RUST_LOG=info,quinn=warn
EXPOSE 8443/udp 8443/tcp
ENTRYPOINT ["/copycat-relay"]
CMD ["run", "--data-dir", "/data", "--listen", "0.0.0.0:8443"]
