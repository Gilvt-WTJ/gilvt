#!/usr/bin/perl
# Turns CHANGELOG.md into the HTML body of /changelog/: released versions only (no [Unreleased]), each
# heading linked to its diff on GitHub. Handles the subset CHANGELOG.md uses: "## [version] - date",
# "### Added" and the like, "- " list items with indented continuation lines, plain paragraphs, and
# inline `code`, **bold** and [links](url). Usage: perl site/changelog.pl CHANGELOG.md
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my @lines = <>;
chomp @lines;
my %refs = map { /^\[([^\]]+)\]:\s*(\S+)/ ? ($1 => $2) : () } @lines;
my @months = qw(January February March April May June July August September October November December);

sub inline {
  my ($text) = @_;
  $text =~ s/&/&amp;/g;
  $text =~ s/</&lt;/g;
  $text =~ s/>/&gt;/g;
  my @code;
  $text =~ s/`([^`]+)`/push @code, $1; "\x00" . $#code . "\x00"/ge;
  $text =~ s/\*\*([^*]+)\*\*/<strong>$1<\/strong>/g;
  $text =~ s/\[([^\]]+)\]\(([^)\s]+)\)/<a href="$2">$1<\/a>/g;
  $text =~ s/\x00(\d+)\x00/<code>$code[$1]<\/code>/g;
  return $text;
}

my ($in_version, $in_list, @para, @item) = (0, 0);
my $out = '';
sub flush_item { if (@item) { $out .= "    <li>" . inline(join ' ', @item) . "</li>\n"; @item = () } }
sub flush_para { if (@para) { $out .= "    <p>" . inline(join ' ', @para) . "</p>\n"; @para = () } }
sub close_list { flush_item(); if ($in_list) { $out .= "    </ul>\n"; $in_list = 0 } }

for my $line (@lines) {
  if ($line =~ /^## \[([^\]]+)\](?:\s*-\s*(\d{4})-(\d\d)-(\d\d))?/) {
    close_list(); flush_para();
    my ($version, $year, $month, $day) = ($1, $2, $3, $4);
    $in_version = $version ne 'Unreleased';
    next unless $in_version;
    (my $id = "v$version") =~ s/\./-/g;
    my $title = $refs{$version} ? qq{<a href="$refs{$version}">$version</a>} : $version;
    $out .= qq{    <h2 id="$id">$title};
    $out .= sprintf qq{ <time datetime="%s-%s-%s">%s %d, %s</time>}, $year, $month, $day, $months[$month - 1], $day, $year
      if defined $year;
    $out .= "</h2>\n";
  } elsif (!$in_version || $line =~ /^\[[^\]]+\]:/) {
    next;
  } elsif ($line =~ /^### (.+)/) {
    close_list(); flush_para();
    $out .= "    <h3>" . inline($1) . "</h3>\n";
  } elsif ($line =~ /^- (.*)/) {
    flush_para(); flush_item();
    $out .= "    <ul>\n" unless $in_list;
    $in_list = 1;
    @item = ($1);
  } elsif ($line =~ /^\s+(\S.*)/ && @item) {
    push @item, $1;
  } elsif ($line =~ /^\s*$/) {
    flush_item(); flush_para();
  } else {
    close_list();
    push @para, $line;
  }
}
close_list(); flush_para();
print $out;
