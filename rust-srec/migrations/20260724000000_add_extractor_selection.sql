-- Which extractor resolves a stream URL, as a config layer.
--
-- `ExtractorFactory::create_extractor` in crates/platforms/src/extractor/factory.rs
-- dispatches on URL regex and only falls through to `StreamlinkExtractor` when no
-- built-in platform claims the URL. That leaves no recourse when a native extractor
-- breaks upstream but the `streamlink` CLI can still resolve the same channel.
--
-- These columns carry the choice through the same four layers as
-- `download_engine`: global -> platform -> template -> streamer (the streamer layer
-- lives in the `streamer_specific_config` JSON blob and needs no column). The value
-- is read by `MergedConfig` and passed to `create_extractor`.
--
-- Recognized values are 'auto' and 'streamlink'; NULL means "inherit from the layer
-- above", and an unset chain resolves to 'auto'. Existing rows stay NULL, so upgrades
-- keep today's URL-regex behaviour.
--
-- Note this is independent of `download_engine`: the extractor finds the stream URL,
-- the engine downloads it. Selecting the streamlink *engine* leaves extraction with
-- the native extractor.

ALTER TABLE global_config
    ADD COLUMN default_extractor TEXT;

ALTER TABLE platform_config
    ADD COLUMN extractor TEXT;

ALTER TABLE template_config
    ADD COLUMN extractor TEXT;
