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

RPM: `sdcompat-notify.spec`.

## 실행

```sh
sdcompat-notify [--ready] [--stopping] [--reloading] [--status=TEXT] [--pid=PID] [VAR=VALUE...]
```

예: 서비스 스크립트에서 준비가 끝났음을 알리기

```sh
sdcompat-notify --ready --status="listening on 8080"
```

`$NOTIFY_SOCKET`이 없으면 오류(종료 코드 1)로 끝납니다.
