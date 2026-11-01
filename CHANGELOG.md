# 변경 이력

[English](CHANGELOG-us.md)

버전은 본체 패키지의 버전입니다. 독립 도구는 버전이 따로 있습니다.
릴리스 노트: [packages/release-notes](packages/release-notes).

## 0.2.0 (2026-11-01)

첫 공개 릴리스(태그 `initial`, 2fac273) 이후의 변경 사항입니다.

### 추가
- s6 부팅의 zram 스왑: zram-generator를 실행하는 oneshot.
- 부팅 시 요청 기반 소켓 활성화.
- `BUS=daemon|broker`: 시스템 버스 선택(dbus-daemon 또는 dbus-broker).
- `SYSLOG=rsyslog|syslog-ng`: s6 부팅의 syslog 데몬 선택.
- `INIT=`는 init 백엔드를, `TOOLS=`는 다른 패키지의 프로그램을 지정합니다.
- s6 서비스 `coredump`: systemd-coredump의 리스너로 코어 덤프를 보관합니다.
- `tests/hardencompare.sh`, `tests/memtest.sh`.

### 변경
- rsyslog, earlyoom, udevd, 시스템 버스를 유닛의 샌드박스 안에서 실행합니다.
- `PrivateUsers=`를 구현했습니다(colord, upower 등 이 설정을 쓰는 서비스가 자체 사용자 네임스페이스를 갖습니다).
- 읽기 전용 마운트를 systemd와 맞췄습니다.
- `/var/log/uinitconverter`는 0750 root:wheel입니다.
- systemd와 같이 하드 `RLIMIT_NOFILE`은 524288, 소프트 `RLIMIT_CORE`는 unlimited입니다.
- Type=notify 도우미는 fork로 남지 않고 다시 exec합니다.
- `OnCalendar=`의 시간대는 `TZ` 전환 대신 jiff를 쓰므로, `uinitconverterd`가
  이름이 지정된 시간대의 다음 실행 시각을 직접 계산합니다.
- 소스: rustfmt 적용, 큰 파일을 역할별로 분리. 로직 변경 없음.

### 수정
- 종료 시 systemd처럼 세션 scope를 `user@UID`보다 먼저 멈춥니다. gnome-shell의
  종료 시 충돌을 해결하기 위한 수정이며, 알려진 문제를 참고하세요.
- DST 변경 전후의 `OnCalendar=`가 systemd와 같아졌습니다. `minutely`가 반복되는
  시간의 두 번째 구간을 건너뛰지 않고, 반복되는 시각이 두 번 실행되지 않으며,
  시간대에서 건너뛴 날짜가 다른 날로 옮겨지지 않습니다.
- 소프트 `RLIMIT_CORE`가 kernel_t에서 init_t로 전환될 때 유지됩니다.
- rsyslog를 샌드박스에 넣기 전에 `/run/user`를 만듭니다.
- `selinux`: restorecon이 호출마다 file_contexts를 불러오고 해제합니다.
- 보안 검토에 따른 입력 검사: 임시 유닛, fd 저장소, runner. linkwatch는 자신과
  같은 사용자의 대기 프로세스에만 동작합니다.

### 제거
- `AUTO_MASK=`.

### 알려진 문제
- 종료 순서 변경으로 gnome-shell 충돌이 해결되는지는 확인되지 않았습니다.
  지금까지 확인한 정상 종료는 2회뿐이고, 로그인 화면에서의 종료는 없었습니다.
- `tests/hardencompare.sh`는 rtkit을 여전히 s6에서 더 약하다고 보고합니다(`ns_mnt`). 이 유닛은
  `PrivateNetwork=`만 설정하므로 파일 시스템 제한이 빠진 것은 아닙니다.
