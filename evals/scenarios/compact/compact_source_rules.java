//- /compact/NoMain.java
/*@pkg*/package compact;

void /*@nomain*/helper() {}
//- /compact/Args.java
static void main(String[] args) {
    IO.println(args.length);
}
//- /other/User.java
package other;

class User {
    /*@ref*/Args a;
}
//? diag compact-source-file pkg
//? diag compact-source-file nomain
//? diag unresolved-type ref
//? diag exact
