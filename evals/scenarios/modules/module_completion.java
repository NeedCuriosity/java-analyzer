//- /src/module-info.java
module r2.app {
    requires java.sq/*@requires*/
    exports exp/*@exports*/
}
//- /src/exported/Api.java
package exported;

public class Api {}
//- /app/Use.java
import module java.ba/*@import*/
class Use {}
//- /app/Complete.java
import module java.sql;
class Complete {
    Conn/*@type*/
}
//? complete type has Connection
//? complete requires has sql
//? complete exports has exported
//? complete import has base
