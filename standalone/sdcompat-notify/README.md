# sdcompat-notify

[English](README-us.md) | **한국어**

`systemd-notify` 대체입니다. `$NOTIFY_SOCKET`에 있는 소켓(경로, 또는 추상
이름공간이면 `@이름`)으로 sd_notify 메시지 하나를 보냅니다. 이 변수를 설정하는
서비스 관리자라면 어디서든 동작합니다(systemd, sdcompat 등).

sdcompat 프로젝트에서 떼어 낸 독립 도구입니다. 다른 sdcompat 구성 요소 없이
빌드·설치·실행됩니다. 의존성은 libc뿐입니다.

## 빌드

```sh
cargo build --release
```

## 설치

```sh
sudo install -Dm0755 target/release/sdcompat-notify /usr/libexec/sdcompat/sdcompat-notify
```

RPM: `./build-rpm.sh [OUTDIR]`가 `sdcompat-notify.spec`을 `~/rpmbuild-sdcompat-standalone/rpms`에
빌드합니다(sudo 불필요. 빌드할 때마다 릴리스가 새로워져 `sudo dnf upgrade`로
올릴 수 있습니다). 설치는 `sudo dnf install ~/rpmbuild-sdcompat-standalone/rpms/sdcompat-notify-*.rpm`이고,
그 뒤로 `setup-fedora.sh`는 이 도구를 rpm에 맡깁니다.

## 실행

```sh
sdcompat-notify [옵션] [VAR=VALUE...]
sdcompat-notify --exec [옵션] [VAR=VALUE...] ";" 명령 [인자...]
sdcompat-notify --booted
```

| 옵션 | 보내는 것 / 하는 일 |
|---|---|
| `--ready`, `--stopping` | `READY=1`, `STOPPING=1` |
| `--reloading` | `RELOADING=1`과 `MONOTONIC_USEC=` |
| `--status=TEXT` | `STATUS=TEXT` |
| `--pid[=PID]` | `MAINPID=PID`; `auto`나 `parent`(기본값)는 호출한 프로세스, `self`는 이 프로세스 |
| `--uid=USER` | USER로 보냄 |
| `--fd=N`, `--fdname=NAME` | fd N을 관리자에 맡김(`FDSTORE=1`, `FDNAME=`); `--fd`는 여러 번 쓸 수 있음 |
| `--no-block` | 관리자가 메시지를 읽을 때까지 기다리지 않음 |
| `--exec` | 보낸 뒤 자기 자리에서 명령을 실행 |
| `--booted` | `/run/systemd/system`이 있으면 종료 코드 0, 없으면 1 |

systemd-notify(254 이후)처럼 메시지는 호출한 프로세스의 이름으로 나가고(커널이
허락하는 경우 자격 증명에 그 PID; 다른 프로세스를 지정하려면 root여야 함),
이어서 `BARRIER=1`로 관리자가 읽을 때까지 기다립니다(최대 5초, 응답이 없으면
경고만 합니다).

예: 서비스 스크립트에서 준비가 끝났음을 알리기

```sh
sdcompat-notify --ready --status="listening on 8080"
```

`$NOTIFY_SOCKET`이 없으면 오류(종료 코드 1)로 끝납니다.
