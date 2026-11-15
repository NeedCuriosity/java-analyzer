//- /lib/Token.java
package lib;

public class /*@cls*/Token {
    public Token() {}
    public static Token of() { return new Token(); }
}
//- /app/Lexer.java
package app;

import lib.Token;
import java.util.List;

class Lexer {
    List<Token> tokens;
    Token next() { return /*@use*/Token.of(); }
    String Token() { return "method named like the class"; }
}
//- /lib/Other.java
package lib;

class Other {}
//? rename cls Lexeme ; has "public class Lexeme {" ; has "public Lexeme() {}" ; has "return new Lexeme();" ; has "import lib.Lexeme;" ; has "List<Lexeme> tokens;" ; has "Lexeme next() { return Lexeme.of(); }" ; has "String Token()"
//? rename use Lexeme ; has "public class Lexeme {" ; has "import lib.Lexeme;"
//? rename cls Other ; fails
