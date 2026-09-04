#!/usr/bin/perl

###################################################################################
# process_version.pl - Generates BletchMAME version information in various forms  #
###################################################################################

# read a line from stdin and extract version
my $input = <STDIN>;
$input =~ s/\r//;
$input =~ s/\n//;
if ($input !~ /^v([0-9]+)\.([0-9]+)(?:-([0-9]+)-g[0-9a-f]+)?(?:-dirty)?$/) {
    die "Cannot process build string: $input";
}
my $major = $1;
my $minor = $2;
my $build = defined($3) ? $3 : '';
my $delim = '.';

if ($build eq "") {
	print join($delim, ($major, $minor)) . "\n";
}
else {
	print join($delim, ($major, $minor, $build)) . "\n";
}
