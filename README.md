<h1 align="center">Personal code review tracking</h1>

If you work on a small to medium sized project, I believe you should probably
be reading every commit the gets merged to master.  I don't mean you actually
have to review the diff - just read the description and glance at the diffstat.
It helps you maintain some sense of who's doing what, and what's going on.
(If you work at a giant company with a monorepo, this document is not going
to be very applicable to you - sorry!)

Once upon a time, I would pull and then check to see what had come down the
pipe, like so:

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

Tips:

* You can edit notes interactively like so: `git notes edit <commit>`.
* You can namespace your notes with `--ref` if you want to keep them organised.
* I like make my notes look like a ["trailer"], just so tools such as tig
  highlight them nicely.

Now, the notes show up in `git log` and you can see which commits you've
looked at and which are waiting to be reviewed.

```commit
$ git show b76598d
commit b76598d2028868fe70d0e038a2841caa6e477d23
Author: Joe Smith <joe@smith.net>
Date:   Fri Jan 8 08:45:16 2021 +0900

    Switch from gcc-9 to gcc-10 on macOS

Notes:
    Reviewed-by: Alex Sayers <alex@asayers.com>
```

The way I think of "reviewed-by" is this: it's like the read/unread status
on your emails.  It doesn't imply approval, disapproval, or any judgement
whatsoever.  It just means that I've looked at the commit.

Remember: these comments aren't indended to be seen by anyone else; they're
just for my own personal use.  If you want to give authors feedback about
thier commits, you should use github/gitlab/trac/gerrit/redmine/the mailing
list/whatever.  This is all about _local, private_ review tracking.

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
Author: Alex Sayers <alex@asayers.com>
Date:   Fri Feb 12 19:09:27 2021 +0900

    Document the CLI options

 src/main.rs | 33 +++++++++++++++++++++++++++++----
 1 file changed, 29 insertions(+), 4 deletions(-)

> mark
da05da11960b59249a286999612c1fcba90dbd19: Reviewed-by: Alex Sayers <alex@asayers.com>

commit 251ec84e30bbed2599f80103dd59a5b850096666
Author: Alex Sayers <alex@asayers.com>
Date:   Fri Feb 12 19:14:33 2021 +0900

    Replace coloured with yansi

 Cargo.lock       | 12 ------------
 Cargo.toml       |  1 -
 src/review_db.rs | 12 ++++++++----
 3 files changed, 8 insertions(+), 17 deletions(-)

> mark Tested
251ec84e30bbed2599f80103dd59a5b850096666: Tested-by: Alex Sayers <alex@asayers.com>
```
