### Summary

Password sheet keeps the old error after a successful retry: once the password is right and the server asks for the verification code, the code sheet still says "That password didn't work".

### Expected

After a wrong password and then the right one, the next sheet (the verification code) shows no error.

### Actual

The verification code sheet shows "That password didn't work. Try again." under its field, though the password was accepted.

### Platform

Ubuntu 24.04, GNOME on Wayland

### Dromon revision

c78d79c

### Fixture

```json fixture
{
  "version": 1,
  "now": "2026-11-25T14:00:00Z",
  "hosts": [
    {"alias": "lab", "source": "ssh-config", "user": "kbd", "hostname": "lab.example.com"}
  ],
  "servers": {
    "lab": {
      "home": "/home/kbd",
      "auth": [
        {"password": {"prompt": "(kbd@lab.example.com) Password: ", "accept": "hunter2"}},
        {"code": {"prompt": "(kbd@lab.example.com) Verification code: ", "accept": "424242"}}
      ],
      "tree": {
        "/home/kbd": [
          {"name": "results.csv", "kind": "file", "size": 88213, "mtime": "2026-11-25T12:30:00Z"}
        ]
      }
    }
  }
}
```

### Script

```json script
{"events": [
  {"t": 0.5, "type": "tap", "key": "host-lab"},
  {"t": 1.3, "type": "type", "text": "hunter1"},
  {"t": 1.8, "type": "key", "name": "Enter"},
  {"t": 2.8, "type": "type", "text": "hunter2"},
  {"t": 3.4, "type": "key", "name": "Enter"},
  {"t": 4.5, "type": "mark", "name": "end"}
]}
```

### Args

```text args
--at mark:end --every 20
```
