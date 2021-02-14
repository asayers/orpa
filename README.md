<h1 align="center">Tracking seen/unseen commits</h1>

If you work on a small to medium sized project, I believe you should probably
be reading every commit the gets merged to master.  I don't mean you actually
have to review the diff - just read the description and glance at the diffstat.
It helps you maintain some sense of who's doing what, and what's going on.

(If, on the other hand, you work at a giant company with a monorepo, then
I'm sorry: this document is not going to be very applicable to you.)

Once upon a time, I would pull master and check to see what had come down
the pipe like so:

```
$ git pull
$ git log ORIG_HEAD..
```

This works, but if you don't look through the new commits then-and-there you
can lose track of which ones you've looked at.  I wanted a way to track the
"read/unread" status of each commit.

Well, git gives you a way to leave notes on commits.  It's called [`git
notes`].  So I started marking the commits I'd looked at by attaching a note
to them.  You use it like this:

```
$ git notes add b76598d -m 'Reviewed-by: Alex Sayers <alex@asayers.com>'
```

Now, the notes show up in `git log` and you can see which commits you've
looked at and which are waiting to be reviewed.

```
$ git show b76598d
commit b76598d2028868fe70d0e038a2841caa6e477d23
Author: Joe Smith <joe@smith.net>
Date:   Fri Jan 8 08:45:16 2021 +0900

    Switch from gcc-9 to gcc-10 on macOS

Notes:
    Reviewed-by: Alex Sayers <alex@asayers.com>
```

Tips:

* You can edit notes interactively like so: `git notes edit <commit>`.
* You can namespace your notes with `--ref` if you want to keep them organised.
* I like make my notes look like a ["trailer"], just so tools such as tig
  highlight them nicely.

The way I think of "reviewed-by" is this: it's like the read/unread status
on your emails.  It doesn't imply approval, disapproval, or any judgement
whatsoever.  It just means that I've looked at the commit.

And remember: these comments aren't indended to be seen by anyone else;
they're just for my own personal use.  If you want to give authors feedback
about thier commits, you should use github/gitlab/trac/gerrit/redmine/the
mailing list/whatever.  This is all about _local, private_ review tracking.

[`git notes`]: https://git-scm.com/docs/git-notes
["trailer"]: https://git-scm.com/docs/git-interpret-trailers

## Enter orpa

`orpa` is a tool for streamlining this workflow.  It shows you the commits
which don't yet have any notes attached.

```
$ orpa
Current branch: The following commits are awaiting review:

7cc8026 Use the gitlab raw Query API                 1 file changed, 9 insertions(+), 7 deletions(-)
251ec84 Replace coloured with yansi                  3 files changed, 8 insertions(+), 17 deletions(-)
da05da1 Document the CLI options                     1 file changed, 29 insertions(+), 4 deletions(-)

Review them using "orpa review"
```

And it has a "review" mode which lets you quickly blast through the unreviewed
commits, marking them with comments as you go:

```
$ orpa review
commit da05da11960b59249a286999612c1fcba90dbd19
Author: Joe Smith <joe@smith.net>
Date:   Fri Feb 12 19:09:27 2021 +0900

    Document the CLI options

 src/main.rs | 33 +++++++++++++++++++++++++++++----
 1 file changed, 29 insertions(+), 4 deletions(-)

> mark
da05da11960b59249a286999612c1fcba90dbd19: Reviewed-by: Alex Sayers <alex@asayers.com>
```

## Reviewing merge requests

Reviewing commits once they're been merged is good, but perhaps you'd like
to review the incoming changes.  `git notes` allows you to attach comments
to _any_ commit in your repo, whether it's merged to a local branch or just
part of a remote branch.  That means we can use our system to keep track of
which MRs we've looked at.

Suppose there's an MR you want to review; the head of the MR branch is
`aadb1f9`, and the merge-base with the target branch is `563e5fb`.  We can
pass this range to `orpa` like so:

```
$ orpa status 563e5fb..aadb1f9
563e5fb..aadb1f9: The following commits are awaiting review:

9fbc3f8 Make the notes ref configurable              3 files changed, 8 insertions(+), 17 deletions(-)
aadb1f9 Use Lazy instead of OnceCell for CLI opts    1 file changed, 29 insertions(+), 4 deletions(-)

Review them using "orpa review 563e5fb..aadb1f9"
```

## Listing merge requests

Orpa can load the open MRs from your MR tracker and display the unreviewed
commits in the same way.  Currently it only supports gitlab, but support
for other trackers could be added too.

Get an API token for your gitlab instance (read-only API access is enough),
and put a section like this in your local git repository's .git/config file:

```ini
[gitlab]
    url = "gitlab.example.com"
    privateToken = "1234567890abcdefgijk"
    projectId = "1"
    username = "asayers"
```

Grab the latest MRs with `orpa fetch`:

```
$ orpa fetch
Fetching open MRs for project 1 from gitlab.example.com...
```

Now the summary contains some new information:

```
$ orpa
Merge requests with unreviewed commits:
    !84    Add --notes-ref CLI argument (2 unreviewed)

Use "orpa mr" to see the full MR information
```

Let's take a look at it...

```
$ orpa mr 84
merge_request !84
Author: Joe Smith (@jsmith)
Date:   2019-12-10 08:42:20.768 UTC

    Add --notes-ref CLI argument

    v1 563e5fb..aadb1f9 (0/2 reviewed)
```
