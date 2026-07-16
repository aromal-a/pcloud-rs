#!/usr/bin/env ruby
# frozen_string_literal: true

# Validate repository-local Markdown links in the mdBook source. mdBook itself
# renders missing targets without failing, so this check is a separate release
# gate. External URLs and same-page anchors are intentionally out of scope.

require "pathname"

root = Pathname.new(ARGV.fetch(0, "docs/book/src"))
unless root.directory?
  warn "mdBook source directory does not exist: #{root}"
  exit 2
end

failures = []
Dir.glob(root.join("**/*.md")).sort.each do |file|
  source = Pathname.new(file)
  File.read(source, encoding: "UTF-8").scan(/\[[^\]]*\]\(([^)]+)\)/).flatten.each do |raw|
    target = raw.split(/[?#]/, 2).first.to_s
    next if target.empty?
    next if target.match?(/\A(?:https?:|mailto:|#)/)

    target = target.delete_prefix("<").delete_suffix(">")
    resolved = source.dirname.join(target).cleanpath
    failures << "#{source}: #{raw} -> #{resolved}" unless resolved.exist?
  end
end

if failures.empty?
  puts "mdBook local links: ok"
  exit 0
end

warn "broken mdBook local links (#{failures.length}):"
failures.each { |failure| warn "  #{failure}" }
exit 1
