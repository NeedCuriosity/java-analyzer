//- /lib/Token.java
package lib;

public class /*@token*/Token {
    public static final /*@self_ty*/Token /*@eof*/EOF = new /*@new1*/Token("eof");
    public final String text;
    public /*@ctor*/Token(String text) { this.text = text; }
    public static /*@ret*/Token of(String text) { return new /*@new2*/Token(text); }
}
//- /app/Lexer.java
package app;

import lib./*@imp*/Token;
import java.util.List;

class Lexer {
    List</*@targ*/Token> tokens;
    /*@field_ty*/Token last = /*@static_use*/Token./*@eof_use*/EOF;

    boolean isToken(Object o) { return o instanceof /*@inst*/Token; }
}
//? refs token = self_ty new1 ret new2 imp targ field_ty static_use inst
//? refs eof = eof_use
//? diag none
