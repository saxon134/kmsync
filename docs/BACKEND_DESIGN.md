# 鍚庣鎶€鏈璁?
## 1. 鍚庣鑱岃矗

鍚庣璐熻矗鎺у埗闈㈣兘鍔涳紝涓嶅弬涓庡眬鍩熺綉杈撳叆浜嬩欢杞彂銆?
鏍稿績鑱岃矗锛?
- 鐢ㄦ埛璐︽埛銆?- 璁惧娉ㄥ唽涓庣粦瀹氥€?- 璁惧瀵嗛挜鍜岃韩浠界鐞嗐€?- 璁惧鍦ㄧ嚎鐘舵€併€?- 鑷姩鏇存柊 IP 鍜屽€欓€夎繛鎺ュ湴鍧€銆?- 閰嶇疆鍚屾銆?- 瀹炴椂淇′护銆?- Relay 閴存潈鍜岃皟搴︺€?- 瀹㈡埛绔増鏈拰鏇存柊绛栫暐銆?
## 2. 鎶€鏈€夊瀷

| 妯″潡 | 鎺ㄨ崘鎶€鏈?|
| --- | --- |
| API 鏈嶅姟 | Go / Rust / Node.js |
| 瀹炴椂淇′护 | WebSocket / gRPC streaming |
| 鏁版嵁搴?| MySQL |
| 缂撳瓨涓庡湪绾跨姸鎬?| Redis |
| Relay | coturn + 鑷爺 QUIC/TCP relay |
| 娑堟伅闃熷垪 | NATS / Kafka锛屽彲鍚庣疆 |
| 閮ㄧ讲 | Kubernetes / Nomad / VM |
| 鐩戞帶 | Prometheus + Grafana |
| 鏃ュ織 | Loki / Elasticsearch |
| 閾捐矾杩借釜 | OpenTelemetry |

## 3. 鏈嶅姟鎷嗗垎

MVP 鍙互鍏堝仛鍗曚綋鏈嶅姟锛屽唴閮ㄦā鍧楀寲銆傝妯℃墿澶у悗鎷嗗垎锛?
- Auth Service锛氱櫥褰曘€佹敞鍐屻€丱Auth銆乼oken銆?- Device Service锛氳澶囩粦瀹氥€佽澶囦俊鎭€佽澶囧瘑閽ャ€?- Presence Service锛氬湪绾跨姸鎬併€佸績璺炽€両P 鏇存柊銆?- Signaling Service锛歅2P 杩炴帴鍗忓晢銆?- Config Service锛氱敤鎴烽厤缃拰璁惧甯冨眬鍚屾銆?- Relay Control Service锛氫腑缁ч壌鏉冦€侀檺娴併€佽皟搴︺€?- Update Service锛氬鎴风鐗堟湰銆佹洿鏂版笭閬撱€佸彂甯冪伆搴︺€?
## 4. 鏁版嵁妯″瀷

### 4.1 users

| 瀛楁 | 绫诲瀷 | 璇存槑 |
| --- | --- | --- |
| id | uuid | 鐢ㄦ埛 ID |
| email | text | 閭 |
| phone | text | 鎵嬫満鍙凤紝鍙€?|
| password_hash | text | 瀵嗙爜 hash锛屽彲閫?|
| status | text | active / disabled |
| created_at | timestamptz | 鍒涘缓鏃堕棿 |
| updated_at | timestamptz | 鏇存柊鏃堕棿 |

### 4.2 devices

| 瀛楁 | 绫诲瀷 | 璇存槑 |
| --- | --- | --- |
| id | uuid | 璁惧 ID |
| user_id | uuid | 鎵€灞炵敤鎴?|
| name | text | 璁惧鍚嶇О |
| os_type | text | macos / windows / linux |
| os_version | text | 绯荤粺鐗堟湰 |
| app_version | text | 瀹㈡埛绔増鏈?|
| public_key | text | 璁惧鍏挜 |
| status | text | active / revoked |
| created_at | timestamptz | 鍒涘缓鏃堕棿 |
| updated_at | timestamptz | 鏇存柊鏃堕棿 |

### 4.3 device_presence

鍙互瀛樺湪 Redis锛孭ostgreSQL 浠呬繚瀛樻渶鍚庣姸鎬併€?
| 瀛楁 | 绫诲瀷 | 璇存槑 |
| --- | --- | --- |
| device_id | uuid | 璁惧 ID |
| user_id | uuid | 鐢ㄦ埛 ID |
| online | bool | 鏄惁鍦ㄧ嚎 |
| lan_ips | JSON | 灞€鍩熺綉 IP 鍒楄〃 |
| public_ip | inet | 鍏綉 IP |
| port | int | 鐩戝惉绔彛 |
| nat_type | text | NAT 绫诲瀷 |
| relay_region | text | 鎺ㄨ崘 Relay 鍖哄煙 |
| last_seen_at | timestamptz | 鏈€鍚庡績璺?|

### 4.4 device_profiles

| 瀛楁 | 绫诲瀷 | 璇存槑 |
| --- | --- | --- |
| id | uuid | Profile ID |
| user_id | uuid | 鐢ㄦ埛 ID |
| source_device_id | uuid | 婧愯澶?|
| target_device_id | uuid | 鐩爣璁惧 |
| config | JSON | 鏄犲皠銆佹粴杞€侀紶鏍囥€佸壀璐存澘閰嶇疆 |
| version | int | 閰嶇疆鐗堟湰 |
| updated_at | timestamptz | 鏇存柊鏃堕棿 |

### 4.5 sessions

| 瀛楁 | 绫诲瀷 | 璇存槑 |
| --- | --- | --- |
| id | uuid | 浼氳瘽 ID |
| user_id | uuid | 鐢ㄦ埛 ID |
| device_id | uuid | 璁惧 ID |
| refresh_token_hash | text | Refresh token hash |
| expires_at | timestamptz | 杩囨湡鏃堕棿 |
| created_at | timestamptz | 鍒涘缓鏃堕棿 |

## 5. 鏍稿績 API

### 5.1 璁よ瘉

```http
POST /v1/auth/register
POST /v1/auth/login
POST /v1/auth/oauth/callback
POST /v1/auth/refresh
POST /v1/auth/logout
```

### 5.2 璁惧

```http
POST /v1/devices/register
GET  /v1/devices
GET  /v1/devices/{device_id}
PATCH /v1/devices/{device_id}
POST /v1/devices/{device_id}/revoke
```

璁惧娉ㄥ唽璇锋眰鍖呭惈锛?
```json
{
  "name": "Kevin's MacBook",
  "os_type": "macos",
  "os_version": "15.0",
  "app_version": "0.1.0",
  "public_key": "base64-public-key"
}
```

### 5.3 蹇冭烦涓?IP 鏇存柊

```http
POST /v1/devices/{device_id}/heartbeat
```

璇锋眰绀轰緥锛?
```json
{
  "lan_ips": ["192.168.1.12", "10.0.0.8"],
  "listen_port": 49210,
  "nat_type": "unknown",
  "capabilities": {
    "input": true,
    "clipboard_text": true,
    "clipboard_file": false,
    "quic": true,
    "webrtc": false
  }
}
```

鏈嶅姟绔牴鎹姹傛簮 IP 璁板綍鍏綉 IP銆?
### 5.4 閰嶇疆鍚屾

```http
GET  /v1/profiles
PUT  /v1/profiles/{profile_id}
GET  /v1/config/sync?since_version=123
```

### 5.5 淇′护

WebSocket:

```text
wss://api.example.com/v1/signaling?device_id=...
```

娑堟伅绫诲瀷锛?
- `device.online`
- `device.offline`
- `connect.request`
- `connect.accept`
- `connect.reject`
- `candidate.add`
- `session.close`
- `config.changed`

## 6. 鍦ㄧ嚎鐘舵€佽璁?
Presence 浣跨敤 Redis 淇濆瓨锛屽綋鍓嶆湇鍔″湪閰嶇疆 `redis_url` 鍚庝細鐢?`SETEX kmsync:presence:{device_id} 90 <presence-json>` 鍐欏叆锛?
```text
presence:{device_id} -> JSON
user_devices_online:{user_id} -> Set(device_id)
```

TTL锛?
## 6.1 MySQL 鎸佷箙鍖?
褰撳墠鏈嶅姟鍦ㄩ厤缃?`mysql_url` 鍚庝娇鐢?MySQL 淇濆瓨 durable state銆?鍚姩鏃朵細鍒涘缓 `kmsync_server_state` 琛紝骞朵互鍗曡 `JSON` 淇濆瓨鐢ㄦ埛銆佷細璇濄€佽澶囥€?profile銆佷俊浠?session 鍜?Relay token 鐘舵€併€傛病鏈夐厤缃暟鎹簱 URL 鏃讹紝鏈嶅姟淇濈暀
鍐呭瓨/JSON 鏂囦欢妯″紡锛屼究浜庢湰鍦板紑鍙戝拰娴嬭瘯銆?
```sql
CREATE TABLE IF NOT EXISTS kmsync_server_state (
  id text PRIMARY KEY,
  state JSON NOT NULL,
  updated_at timestamptz NOT NULL DEFAULT now()
);
```

- 瀹㈡埛绔瘡 15s 鍒?30s 蹇冭烦銆?- Redis presence TTL 璁剧疆涓?60s 鍒?90s銆?- 瓒呮椂鑷姩瑙嗕负绂荤嚎銆?
鐘舵€佸彉鍖栨帹閫侊細

- 璁惧涓婄嚎鏃跺悜鍚岃处鎴峰湪绾胯澶囧箍鎾€?- 璁惧绂荤嚎鏃跺箍鎾€?- 閰嶇疆鍙樻洿鏃跺箍鎾€?
## 7. 淇′护璁捐

淇′护鍙氦鎹㈣繛鎺ュ厓鏁版嵁锛屼笉鎵胯浇杈撳叆浜嬩欢銆?
杩炴帴璇锋眰锛?
```json
{
  "type": "connect.request",
  "request_id": "uuid",
  "from_device_id": "uuid",
  "to_device_id": "uuid",
  "protocol_versions": ["1.0"],
  "transport": ["quic"],
  "candidates": [
    {
      "type": "lan",
      "ip": "192.168.1.12",
      "port": 49210
    }
  ]
}
```

鍊欓€夊湴鍧€绫诲瀷锛?
- lan
- public
- stun
- relay

## 8. Relay 璁捐

Relay 鐢ㄤ簬鏃犳硶鐩磋繛鏃剁殑鏁版嵁杞彂銆俁elay 涓嶅簲瑙ｅ瘑涓氬姟鍐呭銆?
鑳藉姏锛?
- 鍩轰簬 token 閴存潈銆?- 鎸夌敤鎴枫€佽澶囥€佷細璇濋檺娴併€?- 鍖哄煙璋冨害銆?- 娴侀噺缁熻銆?- 杩囨湡浼氳瘽娓呯悊銆?
Relay token 鑾峰彇锛?
```http
POST /v1/relay/token
```

璇锋眰锛?
```json
{
  "source_device_id": "uuid",
  "target_device_id": "uuid",
  "region_preference": "auto"
}
```

鍝嶅簲锛?
```json
{
  "relay_url": "quic://relay-us-west.example.com:443",
  "token": "short-lived-token",
  "expires_in": 300
}
```

## 9. 瀹夊叏璁捐

### 9.1 API 瀹夊叏

- HTTPS only銆?- Access token 鐭湡鏈夋晥銆?- Refresh token 瀛?hash銆?- 璁惧娉ㄥ唽闇€瑕佺櫥褰曟€併€?- 璁惧鎿嶄綔鍙兘璁块棶鍚岃处鎴疯澶囥€?- 閲嶈鎿嶄綔闇€瑕侀噸鏂拌璇佹垨浜屾纭銆?
### 9.2 璁惧瀹夊叏

- 姣忓彴璁惧鏈夌嫭绔嬪瘑閽ュ銆?- 璁惧瑙ｇ粦鍚庢湇鍔＄鎷掔粷鍏朵俊浠ゅ拰 Relay token銆?- 瀹㈡埛绔敹鍒版挙閿€浜嬩欢鍚庢竻鐞嗘湰鍦板嚟璇併€?
### 9.3 鏁版嵁鏈€灏忓寲

鍚庣涓嶅瓨锛?
- 閿洏杈撳叆鍐呭銆?- 榧犳爣浜嬩欢鍐呭銆?- 鍓创鏉垮唴瀹广€?- 鏂囦欢鍐呭銆?
鍚庣鍙瓨锛?
- 璁惧鍏冩暟鎹€?- 鍦ㄧ嚎鐘舵€併€?- 杩炴帴鍊欓€夊湴鍧€銆?- 閰嶇疆銆?- 璇婃柇鎸囨爣銆?
## 10. 鑷姩鏇存柊

鍚庣鎻愪緵鐗堟湰鏌ヨ锛?
```http
GET /v1/updates/check?os=macos&arch=arm64&version=0.1.0&channel=stable
```

鍝嶅簲锛?
```json
{
  "has_update": true,
  "version": "0.2.0",
  "url": "https://download.example.com/app/macos/0.2.0",
  "signature": "base64-signature",
  "release_notes": "Bug fixes and performance improvements.",
  "mandatory": false
}
```

鍙戝竷娓犻亾锛?
- stable銆?- beta銆?- nightly銆?
瀹㈡埛绔洿鏂伴渶瑕佷唬鐮佺鍚嶏細

- macOS Developer ID + notarization銆?- Windows Authenticode銆?- Linux deb/rpm/AppImage 绛惧悕銆?
## 11. 鍙娴嬫€?
鍏抽敭鎸囨爣锛?
- 娉ㄥ唽璁惧鏁般€?- 鍦ㄧ嚎璁惧鏁般€?- 蹇冭烦鎴愬姛鐜囥€?- 淇′护杩炴帴鏁般€?- P2P 鎴愬姛鐜囥€?- Relay 鍥為€€鐜囥€?- Relay 娴侀噺銆?- 鐧诲綍澶辫触鐜囥€?- token refresh 澶辫触鐜囥€?- 閰嶇疆鍚屾鍐茬獊鐜囥€?
鍛婅锛?
- API 閿欒鐜囧崌楂樸€?- Redis 寤惰繜鍗囬珮銆?- 蹇冭烦鍐欏叆澶辫触銆?- 淇′护杩炴帴寮傚父涓嬮檷銆?- Relay 甯﹀鎺ヨ繎涓婇檺銆?- 鏌愬尯鍩?P2P 鎴愬姛鐜囧紓甯镐笅闄嶃€?
## 12. MVP 瀹炵幇寤鸿

绗竴鐗堝悗绔彲浠ョ畝鍖栦负锛?
- 涓€涓?API 鏈嶅姟銆?- MySQL durable state銆?- Redis銆?- 涓€涓?WebSocket 淇′护妯″潡銆?- 鏆備笉瀹炵幇 Relay锛屽彧鏀寔 LAN 鐩磋繛銆?
绗簩鐗堝鍔狅細

- NAT 绌块€忋€?- Relay token銆?- 澶氬尯鍩?Relay銆?- 閰嶇疆澧為噺鍚屾銆?
绗笁鐗堝鍔狅細

- 鐏板害鏇存柊銆?- 璁惧瀹夊叏瀹¤銆?- 缁勭粐璐︽埛鍜屽洟闃熻澶囩鐞嗐€?