# Feature verify PASS tables

## Dev (`px migrate` + `px dev`) — 2026-07-25
```
Verifying features at http://127.0.0.1:3000
PASS   render.spa mode=spa
PASS   render.islands mode=islands
PASS   render.ssr mode=ssr
PASS   plugin.hello {"message":"smoke-hello"}
PASS   features.sse 
PASS   features.ws 
PASS   features.metrics 
PASS   features.password.hash 
PASS   features.password.verify 
PASS   features.password.reject 
PASS   features.jwt.token 
PASS   features.jwt.me 
PASS   features.auth.admin 
PASS   features.auth.forbidden 
PASS   features.jwt.unauthorized 
PASS   features.storage 
PASS   features.storage.traverse 
PASS   features.queue 
PASS   features.mail 
PASS   notes.create 

SUMMARY pass=20 fail=0 skip=0
```

## Release (`px release --version 0.2.0` + `bin/render-modes-smoke serve`) — 2026-07-25
```
Verifying features at http://127.0.0.1:3000
PASS   render.spa mode=spa
PASS   render.islands mode=islands
PASS   render.ssr mode=ssr
PASS   plugin.hello {"message":"smoke-hello"}
PASS   features.sse 
PASS   features.ws 
PASS   features.metrics 
PASS   features.password.hash 
PASS   features.password.verify 
PASS   features.password.reject 
PASS   features.jwt.token 
PASS   features.jwt.me 
PASS   features.auth.admin 
PASS   features.auth.forbidden 
PASS   features.jwt.unauthorized 
PASS   features.storage 
PASS   features.storage.traverse 
PASS   features.queue 
PASS   features.mail 
PASS   notes.create 

SUMMARY pass=20 fail=0 skip=0
```

## Plugin CLI greet
- dev: `cargo run -- greet` → smoke-hello
- release: `./bin/render-modes-smoke greet` → smoke-hello
