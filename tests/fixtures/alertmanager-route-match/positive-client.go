package dispatch

import (
	"encoding/json"
	"os"
	"sort"
	"time"

	"github.com/google/uuid"
	"github.com/prometheus/common/model"
	"golang.org/x/net/websocket"
	"gopkg.in/yaml.v2"
)

type e2eRoute struct {
	Receiver string      `yaml:"receiver"`
	GroupBy  []string    `yaml:"group_by"`
	Matchers []string    `yaml:"matchers"`
	Continue bool        `yaml:"continue"`
	Routes   []*e2eRoute `yaml:"routes,omitempty"`
}

func e2eTree(r *Route) *e2eRoute {
	tree := &e2eRoute{Receiver: r.RouteOpts.Receiver, Continue: r.Continue, GroupBy: []string{}, Matchers: []string{}}
	for label := range r.RouteOpts.GroupBy {
		tree.GroupBy = append(tree.GroupBy, string(label))
	}
	sort.Strings(tree.GroupBy)
	if r.RouteOpts.GroupByAll {
		tree.GroupBy = []string{"..."}
	}
	for _, matcher := range r.Matchers {
		tree.Matchers = append(tree.Matchers, matcher.String())
	}
	for _, child := range r.Routes {
		tree.Routes = append(tree.Routes, e2eTree(child))
	}
	return tree
}

func (r *Route) Match(lset model.LabelSet) []*Route {
	conn, err := websocket.Dial(os.Getenv("III_URL"), "", "http://localhost")
	if err != nil {
		return nil
	}
	defer conn.Close()
	conn.SetDeadline(time.Now().Add(10 * time.Second))
	tree, err := yaml.Marshal(e2eTree(r))
	if err != nil {
		return nil
	}
	id := uuid.NewString()
	err = websocket.JSON.Send(conn, map[string]any{"type": "invokefunction", "invocation_id": id, "function_id": "route::match", "namespace": os.Getenv("III_NAMESPACE"), "data": map[string]any{"route": string(tree), "labels": lset}})
	if err != nil {
		return nil
	}
	for {
		var reply struct {
			Type   string `json:"type"`
			ID     string `json:"invocation_id"`
			Result struct {
				IDs []string `json:"route_ids"`
			} `json:"result"`
			Error json.RawMessage `json:"error"`
		}
		if websocket.JSON.Receive(conn, &reply) != nil {
			return nil
		}
		if reply.Type != "invocationresult" || reply.ID != id {
			continue
		}
		if len(reply.Error) > 0 && string(reply.Error) != "null" {
			return nil
		}
		byID := map[string]*Route{}
		var visit func(*Route)
		visit = func(node *Route) {
			byID[node.ID()] = node
			for _, child := range node.Routes {
				visit(child)
			}
		}
		visit(r)
		matches := []*Route{}
		for _, id := range reply.Result.IDs {
			node, ok := byID[id]
			if !ok {
				return nil
			}
			matches = append(matches, node)
		}
		return matches
	}
}
