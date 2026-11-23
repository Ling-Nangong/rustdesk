#!/usr/bin/env bash
# NOTE: `mediacodec` 是 Android 硬件解码的总开关，上游默认不启用。
# 启用后 scrap 会编译 common/mediacodec.rs（MediaCodecDecoder：直连 ndk MediaCodec，
# ByteBuffer 进出，不依赖 Surface），codec.rs 里对应的解码分支才会真正参与编译。
# hwcodec 仍然保留：编码侧（ram_encode）和将来可能的 Surface 硬解都要用它。
cargo ndk --platform 21 --target aarch64-linux-android build --locked --release --features flutter,hwcodec,mediacodec
