#!/usr/bin/perl

###################################################################################
# process_version.pl - Generates BletchMAME version information in various forms  #
###################################################################################

# read a line from stdin and extract version
my $input = <STDIN>;
if (!defined $input || $input !~ /v([0-9]+)\.([0-9]+)(\-[0-9]+)?/) {
    die "Cannot process build string";
}
my $major = $1;
my $minor = $2;
my $build = $3 // '';
$build =~ s/^\-//;
my $delim = '.';

if ($build eq "") {
	print join($delim, ($major, $minor)) . "\n";
}
else {
	print join($delim, ($major, $minor, $build)) . "\n";
}
