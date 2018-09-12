# `orpa` - a tool for enforcing code review requirements

## MAINTAINERS file

Begin by making a file called MAINTAINERS.  This file outlines the review
requirements for various parts of your source tree.  The format is: pattern,
scrutiny level, number of reviewers, potential reviewers.  For instance:

    # All changes to Cargo.toml go through alice
    Cargo.toml 		!	1	alice

    # Source files should be looked at by _someone_
    src/*			!	1	alice,bob,charlie,daisuke

    # Changes to the backend should be reviewed by two people
    src/backend/*	!	2	alice,bob,charlie,daisuke

    # Changes to protobuf schemas should be reviewed carefully
    *.proto			!!	1	alice,charlie

## Workflow example

Suppose Bob has been working on a branch ("bobs-feature") and wants to get it
merged.  _orpa_ can tell us us whether a commit passes or not, given the rules
defined in MAINTAINERS.  Assuming "bobs-feature" is checked out, we get:

    $ orpa status
    The following requirements are unmet.
    src/main.rs:
        src/*			!	1	alice,bob,charlie,daisuke
    src/schema.proto:
        src/*			!	1	alice,bob,charlie,daisuke
        *.proto			!!	1	alice,charlie,eddy

Looks like some of Bob's changes haven't been accepted, and he needs to get in
touch with some maintainers to get their approval.

_orpa_ can also tell us which branches require our approval.  For instance,
Alice would see this:

    $ orpa todo
    The following branches require review:
    bobs-feature:
        src/main.rs
        src/schema.proto

Suppose Alice has looked at the changes in the "my-feature" branch and is happy
with them.  She can approve them like so:

    $ orpa approve bobs-feature:src/main.rs bobs-feature:src/schema.proto

Or, if "my-feature" is checked out, she can simply do:

    $ orpa approve src/*

Now "bobs-feature" is good-to-go!

    $ orpa status bobs-feature
    All changes approved

## Discussion

`orpa status <branch>` exits with status 0 if "branch" is accepted and 1 if
not, so you can use it in a pre-recieve hook to enforce review policy.

Approvals are stored in git-notes and committed to "refs/notes/orpa".  If
you're so inclined, you can look at the raw approvals data with `git notes
--ref=refs/notes/orpa show <blob>`.  "refs/notes/orpa" is synchronised with
"origin" automatically by `orpa approve`.

A review at a high level of scrutiny (eg. "!!") satifies a requirement for a
review at a lower level of scrutiny (eg. "!").
