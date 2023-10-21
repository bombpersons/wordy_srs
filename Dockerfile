# Build our rust app
FROM rust:1.70 as rust_builder
WORKDIR /src
COPY . .
RUN cargo install --path .

# Build jumanpp 
FROM debian:sid-slim

ARG JPP_VERSION=2.0.0-rc4
ARG JPP_PATH=/usr/local
ENV PATH=${JPP_PATH}/bin:$PATH

WORKDIR /app

RUN apt-get update
RUN apt-get install -y --no-install-recommends \
    curl g++ make cmake xz-utils ca-certificates

WORKDIR ./jumanpp
ADD https://github.com/ku-nlp/jumanpp/releases/download/v${JPP_VERSION}/jumanpp-${JPP_VERSION}.tar.xz jumanpp_src.tar.xz
RUN tar xf jumanpp_src.tar.xz

WORKDIR ./build
RUN cmake ../jumanpp-${JPP_VERSION} \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX=${JPP_PATH}
RUN make install

WORKDIR /app

# Copy over the built rust app to this image.
COPY --from=rust_builder /usr/local/cargo/bin/wordy_srs /app

CMD ["/app/wordy_srs"]
#CMD ls /app/bin -a
EXPOSE 8000