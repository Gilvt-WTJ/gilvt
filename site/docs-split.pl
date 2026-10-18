#!/usr/bin/perl
# Splits the built user guide (/docs/index.html or /zh-CN/docs/index.html, as build.sh's guide() leaves it)
# into one page per Part 2 section, so each topic has its own URL, title and description in search results.
# The Part 1 sections and the hero stay on /docs/, followed by a list of the topics; the contents rail is on
# every page and points at the new URLs. Old links such as /docs/#sessions or /docs/#restore are sent on
# to the page that now holds that id by a few lines of script on /docs/.
# Usage: perl site/docs-split.pl dist-site/docs/index.html en   (or .../zh-CN/docs/index.html zh-CN)
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));
use File::Basename qw(dirname);

my ($index, $lang) = @ARGV;
die "usage: docs-split.pl <docs/index.html> <en|zh-CN>\n" unless $index && $lang && $lang =~ /^(en|zh-CN)$/;
my $site = 'https://gilvt.com';
my ($base, $alt_base, $alt_lang) = $lang eq 'en' ? ('/docs/', '/zh-CN/docs/', 'zh-CN') : ('/zh-CN/docs/', '/docs/', 'en');
my $guide_name = $lang eq 'en' ? 'gilvt User Guide' : 'gilvt 产品手册';

# Readable URLs for the section ids; a section missing here keeps its id as the URL.
my %slug = (
  quicklook => 'quick-look', markdown => 'markdown-mermaid', finder => 'file-search', cli => 'command-line',
  agents => 'agent-sessions', notify => 'notifications', theme => 'themes', ssh => 'ssh-remote',
  config => 'configuration', keys => 'keyboard-shortcuts', limits => 'troubleshooting', layout => 'tabs-and-splits',
);

open my $in, '<', $index or die "docs-split.pl: $index: $!\n";
my $html = do { local $/; <$in> };
close $in;

my ($pre, $hero, $body, $post) = $html =~ m{\A(.*?<main>\n)(<header class="hero">.*?</header>\n)(.*?)(<footer>.*)\z}s
  or die "docs-split.pl: $index: no <main>, hero or <footer>\n";
# Greedy: the last part-head starts Part 2.
my ($part1, $part2_head, $part2) = $body =~ m{\A(.*)(<div class="part-head">[^<]*</div>\n)(.*)\z}s;
die "docs-split.pl: $index: Part 2 has no part-head\n" unless defined $part2;

my @topics;
while ($part2 =~ m{(<section id="([^"]+)">.*?</section>\n)}sg) {
  my ($section, $id) = ($1, $2);
  my ($h2) = $section =~ m{<h2[^>]*>(.*?)</h2>}s or die "docs-split.pl: section $id has no <h2>\n";
  # Described by its first paragraph, or by its text when it has none (a table of shortcuts).
  my ($p) = $section =~ m{<p>(.*?)</p>}s;
  unless (defined $p) {
    ($p = $section) =~ s{<h2[^>]*>.*?</h2>}{}s;
    $p =~ s{<thead>.*?</thead>}{}s;
    $p = $h2 . ($lang eq 'en' ? ': ' : '：') . $p;
  }
  my @ids = $section =~ m{\bid="([^"]+)"}g;
  push @topics, { id => $id, slug => $slug{$id} // $id, section => $section, title => text($h2),
                  description => summary(text($p)), ids => \@ids };
}
die "docs-split.pl: $index: no sections in Part 2\n" unless @topics;
my %page_of = map { my $t = $_; map { ($_ => $t->{slug}) } @{ $t->{ids} } } @topics;
my %is_section = map { ($_->{id} => 1) } @topics;

sub text {
  my ($s) = @_;
  $s =~ s{</tr>}{$lang eq 'en' ? '; ' : '；'}ge;
  $s =~ s{</t[dh]>}{ }g;
  $s =~ s/<[^>]+>//g;
  $s =~ s/&lt;/</g; $s =~ s/&gt;/>/g; $s =~ s/&quot;/"/g; $s =~ s/&amp;/&/g;
  $s =~ s/\s+/ /g;
  $s =~ s/^[ ;；]+|[ ;；]+$//g;
  $s =~ s/ ;/;/g;
  $s =~ s/ ?([；：，。]) ?/$1/g;
  return $s;
}
sub summary {  # a meta description: about 155 characters of English or 80 of Chinese, cut at a word or sentence
  my ($s) = @_;
  my $max = $lang eq 'en' ? 155 : 80;
  return $s if length $s <= $max;
  $s = substr $s, 0, $max;
  if ($lang eq 'en') {
    $s =~ s/\s+\S*$//;
  } elsif ($s =~ /^(.{40,}[，。；、：;])/) {  # Chinese: end at the last punctuation, unless that loses half
    $s = $1;
  }
  $s =~ s/[\s,;:，。；、：]+$//;
  return "$s…";
}
sub attr { my ($s) = @_; $s =~ s/&/&amp;/g; $s =~ s/"/&quot;/g; $s =~ s/</&lt;/g; $s =~ s/>/&gt;/g; return $s }

# The head, site bar and contents rail for one page; $slug is '' for /docs/ itself.
sub frame {
  my ($slug, $title, $description) = @_;
  my $p = $pre;
  my ($url, $alt_url) = ("$site$base$slug" . ($slug ? '/' : ''), "$site$alt_base$slug" . ($slug ? '/' : ''));
  my $en_url = $lang eq 'en' ? $url : $alt_url;
  my $full_title = $slug ? "$title · $guide_name" : $guide_name;
  $p =~ s{<title>[^<]*</title>}{'<title>' . attr($full_title) . '</title>'}e;
  $p =~ s{(<meta name="description" content=")[^"]*}{$1 . attr($description)}e;
  $p =~ s{(<meta property="og:description" content=")[^"]*}{$1 . attr($description)}e;
  $p =~ s{(<meta property="og:title" content=")[^"]*}{$1 . attr($full_title)}e;
  $p =~ s{(<link rel="canonical" href=")[^"]*}{$1$url};
  $p =~ s{(<meta property="og:url" content=")[^"]*}{$1$url};
  $p =~ s{(<link rel="alternate" hreflang="\Q$lang\E" href=")[^"]*}{$1$url};
  $p =~ s{(<link rel="alternate" hreflang="\Q$alt_lang\E" href=")[^"]*}{$1$alt_url};
  $p =~ s{(<link rel="alternate" hreflang="x-default" href=")[^"]*}{$1$en_url};
  # The site bar's language switch goes to the same topic in the other language.
  my $alt_path = $alt_base . ($slug ? "$slug/" : '');
  $p =~ s{(<a class="lang" href=")\Q$alt_base\E"}{$1$alt_path"};
  # Contents rail: Part 1 ids live on /docs/, the rest on their own pages.
  $p =~ s{<a href="#([^"]+)">}{
    my $id = $1;
    my $target = $page_of{$id};
    my $href = defined $target ? "$base$target/" : "$base#$id";
    my $current = defined $target && $target eq $slug ? ' aria-current="page"' : '';
    qq{<a href="$href"$current>};
  }ge;
  $p =~ s{</head>}{<style>
.toc a[aria-current="page"] { color: var(--ink); background: var(--accent-soft); font-weight: 600; }
.topic { font-family: var(--f-display); font-weight: 900; font-size: var(--step-3); margin: 0 0 12px; letter-spacing: 0.01em; }
.pager { display: flex; justify-content: space-between; gap: 16px; margin: 8px 0 0; padding-top: 20px; border-top: 1px solid var(--rule); }
.pager a { text-decoration: none; max-width: 48%; }
.pager .next { margin-left: auto; text-align: right; }
.pager small { display: block; color: var(--ink-soft); font-size: var(--step--1); }
.topics { list-style: none; padding: 0; display: grid; gap: 10px; }
.topics a { display: block; text-decoration: none; background: var(--panel); border: 1px solid var(--rule); border-radius: 8px; padding: 12px 16px; }
.topics b { color: var(--ink); }
.topics span { display: block; color: var(--ink-soft); font-size: 0.93rem; margin-top: 2px; }
</style>
</head>};
  return $p;
}

sub write_page {
  my ($path, $content) = @_;
  mkdir dirname($path);
  open my $out, '>', $path or die "docs-split.pl: $path: $!\n";
  print $out $content;
  close $out;
}

my $dir = dirname($index);
my ($prev_word, $next_word) = $lang eq 'en' ? ('Previous', 'Next') : ('上一节', '下一节');
for my $i (0 .. $#topics) {
  my $t = $topics[$i];
  # The section's own heading is the page's <h1>.
  (my $section = $t->{section}) =~ s{<h2([^>]*)>(.*?)</h2>}{<h1 class="topic"$1>$2</h1>}s;
  my $pager = '';
  if ($i > 0) {
    my $p = $topics[$i - 1];
    $pager .= qq{<a class="prev" href="$base$p->{slug}/"><small>← $prev_word</small>} . attr($p->{title}) . "</a>";
  }
  if ($i < $#topics) {
    my $n = $topics[$i + 1];
    $pager .= qq{<a class="next" href="$base$n->{slug}/"><small>$next_word →</small>} . attr($n->{title}) . "</a>";
  }
  # On a phone the contents rail would come before the topic on every page: there the part heading links
  # back to the list on /docs/ instead, and the pager moves between topics.
  my $page = frame($t->{slug}, $t->{title}, $t->{description});
  $page =~ s{</head>}{<style>\n.part-head a { color: inherit; text-decoration: none; }\n\@media (max-width: 900px) { .toc { display: none; } }\n</style>\n</head>};
  (my $head = $part2_head) =~ s{<div class="part-head">([^<]*)</div>}{<div class="part-head"><a href="$base#topics">$1</a></div>};
  write_page("$dir/$t->{slug}/index.html",
    $page . $head . "\n" . $section . qq{\n<nav class="pager">$pager</nav>\n\n} . $post);
}

# /docs/ keeps the hero and Part 1, then lists the topics, and forwards old #anchors to their new pages.
my $list = join '', map {
  qq{  <li><a href="$base$_->{slug}/"><b>} . attr($_->{title}) . '</b><span>' . attr($_->{description}) . "</span></a></li>\n"
} @topics;
my $forward = join ',', map { qq{"$_":"$page_of{$_}/} . ($is_section{$_} ? '' : "#$_") . '"' } sort keys %page_of;
my $script = qq{<script>
(function () {
  var pages = {$forward};
  var id = decodeURIComponent(location.hash.slice(1));
  if (pages[id]) location.replace("$base" + pages[id]);
})();
</script>
};
my $intro_description = ($pre =~ m{<meta name="description" content="([^"]*)"})[0];
my $index_page = frame('', $guide_name, $intro_description) . $hero . "\n" . $part1 . $part2_head . "\n<ol class=\"topics\" id=\"topics\">\n$list</ol>\n\n" . $post;
$index_page =~ s{</body>}{$script</body>};
write_page($index, $index_page);
