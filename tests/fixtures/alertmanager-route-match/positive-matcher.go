package main

import (
	"encoding/json"
	"os"
	"sort"

	"github.com/prometheus/alertmanager/config"
	"github.com/prometheus/alertmanager/dispatch"
	"github.com/prometheus/common/model"
	"gopkg.in/yaml.v2"
)

func main() {
	var input struct {
		Route  string         `json:"route"`
		Labels model.LabelSet `json:"labels"`
	}
	if json.Unmarshal([]byte(os.Args[1]), &input) != nil {
		os.Exit(1)
	}
	var route config.Route
	if yaml.Unmarshal([]byte(input.Route), &route) != nil {
		os.Exit(1)
	}
	matches := dispatch.NewRoute(&route, nil).MatchLocal(input.Labels)
	ids, receivers, groups, all := []string{}, []string{}, [][]string{}, []bool{}
	for _, match := range matches {
		ids = append(ids, match.ID())
		receivers = append(receivers, match.RouteOpts.Receiver)
		group := []string{}
		for label := range match.RouteOpts.GroupBy {
			group = append(group, string(label))
		}
		sort.Strings(group)
		groups = append(groups, group)
		all = append(all, match.RouteOpts.GroupByAll)
	}
	json.NewEncoder(os.Stdout).Encode(map[string]any{"route_ids": ids, "receivers": receivers, "group_by": groups, "group_by_all": all})
}
