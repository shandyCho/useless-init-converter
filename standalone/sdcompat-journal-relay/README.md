# sdcompat-journal-relay

[English](README-us.md) | **한국어**

systemd-journald 없이 돌리는 시스템을 위한 도구입니다. journald의 클라이언트
소켓(`/run/systemd/journal/socket`의 native 프로토콜과 `stdout` 스트림)을 열고,
받은 메시지를 syslog(`/dev/log`, 보통 rsyslog)로 넘깁니다. dbus-broker-launch처럼
이 소켓이 없으면 시작하지 않는 프로그램이 있어서 필요합니다.

sdcompat 프로젝트에서 떼어 낸 독립 도구입니다. 다른 sdcompat 구성 요소 없이
빌드·설치·실행됩니다. 의존성은 libc와, 실행할 때 `/dev/log`를 여는 syslog
데몬(rsyslog 등)입니다.

## 빌드

```sh
cargo build --release
cargo test
```

## 설치

```sh
sudo install -Dm0755 target/release/sdcompat-journal-relay /usr/libexec/sdcompat/sdcompat-journal-relay
```

RPM: `./build-rpm.sh [OUTDIR]`가 `sdcompat-journal-relay.spec`을 `~/rpmbuild-sdcompat-standalone/rpms`에
빌드합니다(sudo 불필요. 빌드할 때마다 릴리스가 새로워져 `sudo dnf upgrade`로
올릴 수 있습니다). 설치는 `sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/sdcompat-journal-relay-*.rpm`입니다. `./build-rpm.sh --srpm`은
다른 곳에서 다시 빌드할(`rpmbuild --rebuild`, mock) 소스 RPM만 만듭니다.

## 실행

```sh
sdcompat-journal-relay [--dir DIR] [--syslog PATH] [--ready-fd N]
```

- `--dir DIR`: journald 소켓을 만들 곳(기본값 `/run/systemd/journal`).
- `--syslog PATH`: 넘길 syslog 소켓(기본값 `/dev/log`).
- `--ready-fd N`: 소켓을 연 뒤 fd N에 줄바꿈을 씁니다(s6의 준비 완료 알림).

journald가 도는 시스템에서는 실행하지 마세요(같은 소켓을 씁니다). sdcompat을
`JOURNAL=no`로 설치하면 `journal-relay` s6 서비스가 이것을 실행하며, 이때는 꼭
설치되어 있어야 합니다(`sdcompat init --journal=no`가 확인합니다).

MESSAGE, PRIORITY, SYSLOG_FACILITY, SYSLOG_IDENTIFIER, SYSLOG_PID만 넘기고 나머지
필드는 버립니다. syslog가 없거나 따라오지 못하면 메시지를 버립니다. 클라이언트를
막지 않기 위해서입니다.
