# Alertmanager route-match fixture

Frozen `prometheus/alertmanager` at tag `v0.34.1` (commit
`73c6bfe7393929211294c1954f30d8ed78e4d0ad`). The Git bundle is a depth-1
snapshot of that commit. The upstream URL is provenance.

`oracle.json` is the runner-owned match table. It is not part of the
workspace the subject sees. Cases come from `dispatch/route_test.go`
`TestRouteMatch` and from `config/testdata/conf.good.yml`.

The Alertmanager sources remain under the Apache License 2.0 in
`LICENSE`.
