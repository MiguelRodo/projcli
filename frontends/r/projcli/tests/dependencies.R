library(projcli)
code <- function(expr, expected) {
 got <- tryCatch(force(expr),projcli_error=identity)
 stopifnot(inherits(got,"projcli_error"),identical(got$code,expected),
  !grepl("private-restore-canary",paste(capture.output(str(got)),collapse="")))
}
code(proj_renv_restore(),"invalid_arguments")
decode <- get(".proj_renv_decode",asNamespace("projcli"))
code(decode(c(charToRaw("private-restore-canary"),as.raw(0L)),0L),"renv_restore_failed")
receipt <- list(ok=TRUE,result=list(lockfile_md5=paste(rep("a",32),collapse=""),
 renv_version="1.1.5",packages=list(projclifixture="0.0.1")))
bytes <- function(x) charToRaw(jsonlite::toJSON(x,auto_unbox=TRUE))
stopifnot(decode(bytes(receipt),0L)$packages$projclifixture=="0.0.1")
for(packages in list(list("0.0.1"),list(projclifixture=list("private-restore-canary")),
                    list(projclifixture="https://u:private-restore-canary@host"),
                    structure(list("1","2"),names=c("duplicate","duplicate")))) {
 bad <- receipt;bad$result$packages <- packages
 code(decode(bytes(bad),0L),"renv_restore_failed")
}
bad<-receipt;bad$result$unexpected<-"private-restore-canary"
code(decode(bytes(bad),0L),"renv_restore_failed")
cat("PASS optional restore malformed/NUL receipt redaction and exact package-version schema\n")
