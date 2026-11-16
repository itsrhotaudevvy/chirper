package localmx

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"

	"maunium.net/go/mautrix/id"

	"chirper.dev/core/store"
)

// updateProfile applies mutate to the profile of userID, creating it if needed.
func (c *Connector) updateProfile(ctx context.Context, userID id.UserID, mutate func(*store.Profile)) error {
	c.profileMu.Lock()
	defer c.profileMu.Unlock()
	p, err := c.st.GetProfile(ctx, userID)
	if errors.Is(err, store.ErrNotFound) {
		p = &store.Profile{UserID: userID, ExtraJSON: "{}"}
	} else if err != nil {
		return err
	}
	mutate(p)
	return c.st.SetProfile(ctx, p)
}

// SetDisplayName implements bridgev2.MatrixAPI.
func (i *intent) SetDisplayName(ctx context.Context, name string) error {
	return i.c.updateProfile(ctx, i.mxid, func(p *store.Profile) { p.Displayname = name })
}

// SetAvatarURL implements bridgev2.MatrixAPI.
func (i *intent) SetAvatarURL(ctx context.Context, avatarURL id.ContentURIString) error {
	return i.c.updateProfile(ctx, i.mxid, func(p *store.Profile) { p.AvatarURL = avatarURL })
}

// SetExtraProfileMeta implements bridgev2.MatrixAPI. The data is stored as a JSON object.
func (i *intent) SetExtraProfileMeta(ctx context.Context, data any) error {
	fields, err := jsonObject(data)
	if err != nil {
		return err
	}
	extra, err := json.Marshal(fields)
	if err != nil {
		return fmt.Errorf("failed to marshal profile metadata: %w", err)
	}
	return i.c.updateProfile(ctx, i.mxid, func(p *store.Profile) { p.ExtraJSON = string(extra) })
}

// SetProfile implements bridgev2.MatrixAPI. displayname and avatar_url set the profile fields;
// all other keys are stored as extra metadata.
func (i *intent) SetProfile(ctx context.Context, data any) error {
	fields, err := jsonObject(data)
	if err != nil {
		return err
	}
	name, _ := fields["displayname"].(string)
	avatar, _ := fields["avatar_url"].(string)
	delete(fields, "displayname")
	delete(fields, "avatar_url")
	extra, err := json.Marshal(fields)
	if err != nil {
		return fmt.Errorf("failed to marshal profile metadata: %w", err)
	}
	return i.c.updateProfile(ctx, i.mxid, func(p *store.Profile) {
		p.Displayname = name
		p.AvatarURL = id.ContentURIString(avatar)
		p.ExtraJSON = string(extra)
	})
}

// jsonObject converts data to a generic JSON object.
func jsonObject(data any) (map[string]any, error) {
	raw, err := json.Marshal(data)
	if err != nil {
		return nil, fmt.Errorf("failed to marshal profile data: %w", err)
	}
	var fields map[string]any
	if err := json.Unmarshal(raw, &fields); err != nil {
		return nil, fmt.Errorf("profile data is not a JSON object: %w", err)
	}
	if fields == nil {
		fields = map[string]any{}
	}
	return fields, nil
}
