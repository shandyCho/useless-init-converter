# uinitconverter-fdstore

[English](README-us.md) | **한국어**

서비스의 파일 디스크립터를 보관하는 작은 서버입니다. systemd의 `FDSTORE=1`과
`FileDescriptorStoreMax=`처럼, 서비스가 맡긴 fd와 소켓 활성화 서비스의 리스닝
소켓을 자기 프로세스에 들고 있다가, 서비스나 서비스 관리자가 다시 시작한 뒤
돌려줍니다.

uinitconverter 프로젝트에서 떼어 낸 독립 도구입니다. 설정 파일도 유닛 파일도 읽지
않으며, 다른 uinitconverter 구성 요소 없이 빌드·설치·실행됩니다. 의존성은 libc뿐입니다.

## 빌드

```sh
cargo build --release
cargo test
```

## 설치

```sh
sudo install -Dm0755 target/release/uinitconverter-fdstore /usr/libexec/uinitconverter/uinitconverter-fdstore
```

RPM: `./build-rpm.sh [OUTDIR]`가 `uinitconverter-fdstore.spec`을 `~/rpmbuild-uinitconverter-standalone/rpms`에
빌드합니다(sudo 불필요. 빌드할 때마다 릴리스가 새로워져 `sudo dnf upgrade`로
올릴 수 있습니다). 설치는 `sudo dnf install ~/rpmbuild-uinitconverter-standalone/rpms/uinitconverter-fdstore-*.rpm`입니다. `./build-rpm.sh --srpm`은
다른 곳에서 다시 빌드할(`rpmbuild --rebuild`, mock) 소스 RPM만 만듭니다.

## 실행

```sh
uinitconverter-fdstore [--socket PATH] [--ready-fd N]
```

- `--socket PATH`: 기다릴 소켓(기본값 `/run/uinitconverter/fdstore`).
- `--ready-fd N`: 소켓을 연 뒤 fd N에 줄바꿈을 씁니다(s6의 준비 완료 알림).

root와, 서버를 실행한 사용자만 쓸 수 있고, SELinux에서는 서버와 같은 도메인의
상대만 쓸 수 있습니다(uinitconverter에서는 유일한 클라이언트인 uinitconverterd와 러너의 도메인
`init_t`. 제한된 root 데몬 상당수가 `init_t`의 소켓에 연결할 수 있기 때문입니다).
uinitconverter와 함께 쓸 때는
`/etc/uinitconverter/s6/uinitconverter-fdstore` s6 서비스가 이것을 실행합니다. 이 도구가
없으면 그 서비스는 아무것도 하지 않고, uinitconverterd가 fd를 직접 보관합니다.

## 프로토콜

`SOCK_SEQPACKET` 소켓, 메시지마다 fd 하나(SCM_RIGHTS):

| 요청 | 응답 |
|---|---|
| `PUT\0<키>\0<이름>\0<최대>` + fd | `OK` 또는 `ERR <메시지>` |
| `GET\0<키>` | fd마다 `FD\0<이름>` + fd, 마지막에 `END` |
| `DEL\0<키>[\0<이름>]` | `OK` |

`<최대>`는 그 키가 가질 수 있는 fd 수입니다(서비스의 `FileDescriptorStoreMax=`).
서버가 유닛 파일을 읽지 않도록 클라이언트가 보냅니다. 없으면 `<유닛>#sockets`(1024개)와
`<유닛>#stdio`(3개) 키만 받습니다.
