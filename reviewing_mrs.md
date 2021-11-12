<h1 align="center">Reviewing merge requests</h1>

(This follows on from [Tracking read/unread commits](reviewing_commits.md).)

Reviewing commits once they're been merged is good, but I'd also like to
review incoming changes before they land.  Fortunately, `git notes` allows
you to attach comments to _any_ commit in your repo, whether or not it's
in a local branch.  That means we can use our system to keep track of the
commits on other people's branches too.

## Joe's MR

Joe has an MR open.  He wants to merge 563e5fb..aadb1f9 into master.  I look
through those commits, marking them as seen, and then I send Joe some feedback
on them.

## Re-reviewing merge requests

Joe has read my feedback and updated his MR.  Now it shows the range
7be3424..de31ea2.

These commits have completely different revisions to the ones I reviewed.
How do I tell what changed?  I could go over them again, trying to spot what
changed, but that's error-prone, time-consuming, and (worst) boring!

Once again, git has us covered; this time it's with `git range-diff`:

```
$ git range-diff 563e5fb..aadb1f9 7be3424..de31ea2
1:  9fbc3f82 = 1:  ce0ad59e Make the notes ref configurable
2:  aadb1f9e = 2:  30bb419c Use Lazy for CLI opts
-:  -------- > 3:  de31ea2c Rename --hidden to --all
```

git range-diff compares the old and new ranges and summarises the changes.
In this case, it looks like Joe just rebased and added a commit.  So I can
just mark the first two as seen without thinking, and then take a closer
look at the third one.

## Challenge: remembering what you reviewed

The main challenge with this workflow is keeping track of commit ranges.
Getting the current range is easy enough: your git platform of choice probably
has a web API which will tell you.  If not, you can use `git merge-base`.

However, you do have to remember the commit range as it was the last time
you reviewed the branch.  As far as I can tell, none of the major platforms
will help you with this (eg. by giving all historical ranges).
