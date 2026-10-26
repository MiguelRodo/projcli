library(projcli)
code <- function(expr, expected, label="receipt validation") {
 got <- tryCatch(force(expr),projcli_error=identity)
 if(!inherits(got,"projcli_error")) stop(paste("Expected a redacted error for",label))
 stopifnot(identical(got$code,expected),
  !grepl("private-restore-canary",paste(capture.output(str(got)),collapse="")))
}
code(proj_renv_restore(),"invalid_arguments")
decode <- get(".proj_renv_decode",asNamespace("projcli"))
code(decode(c(charToRaw("private-restore-canary"),as.raw(0L)),0L),"renv_restore_failed")
receipt <- list(ok=TRUE,result=list(lockfile_md5=paste(rep("a",32),collapse=""),
 renv_version="1.1.5",packages=list(projclifixture="0.0.1")))
bytes <- function(x) charToRaw(jsonlite::toJSON(x,auto_unbox=TRUE))
stopifnot(decode(bytes(receipt),0L)$packages$projclifixture=="0.0.1")
malformed <- list(unnamed=list("0.0.1"),
 nested=list(projclifixture=list("private-restore-canary")),
 url=list(projclifixture="https://u:private-restore-canary@host"))
for(label in names(malformed)) {
 bad <- receipt;bad$result$packages <- malformed[[label]]
 code(decode(bytes(bad),0L),"renv_restore_failed",label)
}
# Serializers may normalize duplicate R names. Send actual duplicated JSON keys
# so this tests the native reader instead of assuming a serializer's behavior.
duplicate <- paste0('{"ok":true,"result":{"lockfile_md5":"',paste(rep("a",32),collapse=""),
 '","renv_version":"1.1.5","packages":{"duplicate":"1","duplicate":"2"}}}')
code(decode(charToRaw(duplicate),0L),"renv_restore_failed","literal duplicated JSON keys")
bad<-receipt;bad$result$unexpected<-"private-restore-canary"
code(decode(bytes(bad),0L),"renv_restore_failed")
cat("PASS optional restore malformed/NUL receipt redaction and exact package-version schema\n")
