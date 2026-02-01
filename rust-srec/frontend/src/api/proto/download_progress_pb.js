/*eslint-disable block-scoped-var, id-length, no-control-regex, no-magic-numbers, no-prototype-builtins, no-redeclare, no-shadow, no-var, sort-vars*/
import * as $protobuf from "protobufjs/minimal";

// Common aliases
const $Reader = $protobuf.Reader, $Writer = $protobuf.Writer, $util = $protobuf.util;

// Exported root namespace
const $root = $protobuf.roots["default"] || ($protobuf.roots["default"] = {});

export const download_progress = $root.download_progress = (() => {

    /**
     * Namespace download_progress.
     * @exports download_progress
     * @namespace
     */
    const download_progress = {};

    /**
     * EventType enum.
     * @name download_progress.EventType
     * @enum {number}
     * @property {number} EVENT_TYPE_UNSPECIFIED=0 EVENT_TYPE_UNSPECIFIED value
     * @property {number} EVENT_TYPE_SNAPSHOT=1 EVENT_TYPE_SNAPSHOT value
     * @property {number} EVENT_TYPE_DOWNLOAD_STARTED=2 EVENT_TYPE_DOWNLOAD_STARTED value
     * @property {number} EVENT_TYPE_PROGRESS=3 EVENT_TYPE_PROGRESS value
     * @property {number} EVENT_TYPE_SEGMENT_COMPLETED=4 EVENT_TYPE_SEGMENT_COMPLETED value
     * @property {number} EVENT_TYPE_DOWNLOAD_COMPLETED=5 EVENT_TYPE_DOWNLOAD_COMPLETED value
     * @property {number} EVENT_TYPE_DOWNLOAD_FAILED=6 EVENT_TYPE_DOWNLOAD_FAILED value
     * @property {number} EVENT_TYPE_DOWNLOAD_CANCELLED=7 EVENT_TYPE_DOWNLOAD_CANCELLED value
     * @property {number} EVENT_TYPE_ERROR=8 EVENT_TYPE_ERROR value
     * @property {number} EVENT_TYPE_DOWNLOAD_REJECTED=9 EVENT_TYPE_DOWNLOAD_REJECTED value
     */
    download_progress.EventType = (function() {
        const valuesById = {}, values = Object.create(valuesById);
        values[valuesById[0] = "EVENT_TYPE_UNSPECIFIED"] = 0;
        values[valuesById[1] = "EVENT_TYPE_SNAPSHOT"] = 1;
        values[valuesById[2] = "EVENT_TYPE_DOWNLOAD_STARTED"] = 2;
        values[valuesById[3] = "EVENT_TYPE_PROGRESS"] = 3;
        values[valuesById[4] = "EVENT_TYPE_SEGMENT_COMPLETED"] = 4;
        values[valuesById[5] = "EVENT_TYPE_DOWNLOAD_COMPLETED"] = 5;
        values[valuesById[6] = "EVENT_TYPE_DOWNLOAD_FAILED"] = 6;
        values[valuesById[7] = "EVENT_TYPE_DOWNLOAD_CANCELLED"] = 7;
        values[valuesById[8] = "EVENT_TYPE_ERROR"] = 8;
        values[valuesById[9] = "EVENT_TYPE_DOWNLOAD_REJECTED"] = 9;
        return values;
    })();

    download_progress.WsMessage = (function() {

        /**
         * Properties of a WsMessage.
         * @memberof download_progress
         * @interface IWsMessage
         * @property {download_progress.EventType|null} [eventType] WsMessage eventType
         * @property {download_progress.IDownloadSnapshot|null} [snapshot] WsMessage snapshot
         * @property {download_progress.IDownloadStarted|null} [downloadStarted] WsMessage downloadStarted
         * @property {download_progress.IDownloadProgress|null} [progress] WsMessage progress
         * @property {download_progress.ISegmentCompleted|null} [segmentCompleted] WsMessage segmentCompleted
         * @property {download_progress.IDownloadCompleted|null} [downloadCompleted] WsMessage downloadCompleted
         * @property {download_progress.IDownloadFailed|null} [downloadFailed] WsMessage downloadFailed
         * @property {download_progress.IDownloadCancelled|null} [downloadCancelled] WsMessage downloadCancelled
         * @property {download_progress.IErrorPayload|null} [error] WsMessage error
         * @property {download_progress.IDownloadRejected|null} [downloadRejected] WsMessage downloadRejected
         */

        /**
         * Constructs a new WsMessage.
         * @memberof download_progress
         * @classdesc Represents a WsMessage.
         * @implements IWsMessage
         * @constructor
         * @param {download_progress.IWsMessage=} [properties] Properties to set
         */
        function WsMessage(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * WsMessage eventType.
         * @member {download_progress.EventType} eventType
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.eventType = 0;

        /**
         * WsMessage snapshot.
         * @member {download_progress.IDownloadSnapshot|null|undefined} snapshot
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.snapshot = null;

        /**
         * WsMessage downloadStarted.
         * @member {download_progress.IDownloadStarted|null|undefined} downloadStarted
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.downloadStarted = null;

        /**
         * WsMessage progress.
         * @member {download_progress.IDownloadProgress|null|undefined} progress
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.progress = null;

        /**
         * WsMessage segmentCompleted.
         * @member {download_progress.ISegmentCompleted|null|undefined} segmentCompleted
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.segmentCompleted = null;

        /**
         * WsMessage downloadCompleted.
         * @member {download_progress.IDownloadCompleted|null|undefined} downloadCompleted
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.downloadCompleted = null;

        /**
         * WsMessage downloadFailed.
         * @member {download_progress.IDownloadFailed|null|undefined} downloadFailed
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.downloadFailed = null;

        /**
         * WsMessage downloadCancelled.
         * @member {download_progress.IDownloadCancelled|null|undefined} downloadCancelled
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.downloadCancelled = null;

        /**
         * WsMessage error.
         * @member {download_progress.IErrorPayload|null|undefined} error
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.error = null;

        /**
         * WsMessage downloadRejected.
         * @member {download_progress.IDownloadRejected|null|undefined} downloadRejected
         * @memberof download_progress.WsMessage
         * @instance
         */
        WsMessage.prototype.downloadRejected = null;

        // OneOf field names bound to virtual getters and setters
        let $oneOfFields;

        /**
         * WsMessage payload.
         * @member {"snapshot"|"downloadStarted"|"progress"|"segmentCompleted"|"downloadCompleted"|"downloadFailed"|"downloadCancelled"|"error"|"downloadRejected"|undefined} payload
         * @memberof download_progress.WsMessage
         * @instance
         */
        Object.defineProperty(WsMessage.prototype, "payload", {
            get: $util.oneOfGetter($oneOfFields = ["snapshot", "downloadStarted", "progress", "segmentCompleted", "downloadCompleted", "downloadFailed", "downloadCancelled", "error", "downloadRejected"]),
            set: $util.oneOfSetter($oneOfFields)
        });

        /**
         * Creates a new WsMessage instance using the specified properties.
         * @function create
         * @memberof download_progress.WsMessage
         * @static
         * @param {download_progress.IWsMessage=} [properties] Properties to set
         * @returns {download_progress.WsMessage} WsMessage instance
         */
        WsMessage.create = function create(properties) {
            return new WsMessage(properties);
        };

        /**
         * Encodes the specified WsMessage message. Does not implicitly {@link download_progress.WsMessage.verify|verify} messages.
         * @function encode
         * @memberof download_progress.WsMessage
         * @static
         * @param {download_progress.IWsMessage} message WsMessage message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        WsMessage.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.eventType != null && Object.hasOwnProperty.call(message, "eventType"))
                writer.uint32(/* id 1, wireType 0 =*/8).int32(message.eventType);
            if (message.snapshot != null && Object.hasOwnProperty.call(message, "snapshot"))
                $root.download_progress.DownloadSnapshot.encode(message.snapshot, writer.uint32(/* id 2, wireType 2 =*/18).fork()).ldelim();
            if (message.downloadStarted != null && Object.hasOwnProperty.call(message, "downloadStarted"))
                $root.download_progress.DownloadStarted.encode(message.downloadStarted, writer.uint32(/* id 3, wireType 2 =*/26).fork()).ldelim();
            if (message.progress != null && Object.hasOwnProperty.call(message, "progress"))
                $root.download_progress.DownloadProgress.encode(message.progress, writer.uint32(/* id 4, wireType 2 =*/34).fork()).ldelim();
            if (message.segmentCompleted != null && Object.hasOwnProperty.call(message, "segmentCompleted"))
                $root.download_progress.SegmentCompleted.encode(message.segmentCompleted, writer.uint32(/* id 5, wireType 2 =*/42).fork()).ldelim();
            if (message.downloadCompleted != null && Object.hasOwnProperty.call(message, "downloadCompleted"))
                $root.download_progress.DownloadCompleted.encode(message.downloadCompleted, writer.uint32(/* id 6, wireType 2 =*/50).fork()).ldelim();
            if (message.downloadFailed != null && Object.hasOwnProperty.call(message, "downloadFailed"))
                $root.download_progress.DownloadFailed.encode(message.downloadFailed, writer.uint32(/* id 7, wireType 2 =*/58).fork()).ldelim();
            if (message.downloadCancelled != null && Object.hasOwnProperty.call(message, "downloadCancelled"))
                $root.download_progress.DownloadCancelled.encode(message.downloadCancelled, writer.uint32(/* id 8, wireType 2 =*/66).fork()).ldelim();
            if (message.error != null && Object.hasOwnProperty.call(message, "error"))
                $root.download_progress.ErrorPayload.encode(message.error, writer.uint32(/* id 9, wireType 2 =*/74).fork()).ldelim();
            if (message.downloadRejected != null && Object.hasOwnProperty.call(message, "downloadRejected"))
                $root.download_progress.DownloadRejected.encode(message.downloadRejected, writer.uint32(/* id 10, wireType 2 =*/82).fork()).ldelim();
            return writer;
        };

        /**
         * Encodes the specified WsMessage message, length delimited. Does not implicitly {@link download_progress.WsMessage.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.WsMessage
         * @static
         * @param {download_progress.IWsMessage} message WsMessage message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        WsMessage.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a WsMessage message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.WsMessage
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.WsMessage} WsMessage
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        WsMessage.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.WsMessage();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.eventType = reader.int32();
                        break;
                    }
                case 2: {
                        message.snapshot = $root.download_progress.DownloadSnapshot.decode(reader, reader.uint32());
                        break;
                    }
                case 3: {
                        message.downloadStarted = $root.download_progress.DownloadStarted.decode(reader, reader.uint32());
                        break;
                    }
                case 4: {
                        message.progress = $root.download_progress.DownloadProgress.decode(reader, reader.uint32());
                        break;
                    }
                case 5: {
                        message.segmentCompleted = $root.download_progress.SegmentCompleted.decode(reader, reader.uint32());
                        break;
                    }
                case 6: {
                        message.downloadCompleted = $root.download_progress.DownloadCompleted.decode(reader, reader.uint32());
                        break;
                    }
                case 7: {
                        message.downloadFailed = $root.download_progress.DownloadFailed.decode(reader, reader.uint32());
                        break;
                    }
                case 8: {
                        message.downloadCancelled = $root.download_progress.DownloadCancelled.decode(reader, reader.uint32());
                        break;
                    }
                case 9: {
                        message.error = $root.download_progress.ErrorPayload.decode(reader, reader.uint32());
                        break;
                    }
                case 10: {
                        message.downloadRejected = $root.download_progress.DownloadRejected.decode(reader, reader.uint32());
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a WsMessage message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.WsMessage
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.WsMessage} WsMessage
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        WsMessage.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a WsMessage message.
         * @function verify
         * @memberof download_progress.WsMessage
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        WsMessage.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            let properties = {};
            if (message.eventType != null && message.hasOwnProperty("eventType"))
                switch (message.eventType) {
                default:
                    return "eventType: enum value expected";
                case 0:
                case 1:
                case 2:
                case 3:
                case 4:
                case 5:
                case 6:
                case 7:
                case 8:
                case 9:
                    break;
                }
            if (message.snapshot != null && message.hasOwnProperty("snapshot")) {
                properties.payload = 1;
                {
                    let error = $root.download_progress.DownloadSnapshot.verify(message.snapshot);
                    if (error)
                        return "snapshot." + error;
                }
            }
            if (message.downloadStarted != null && message.hasOwnProperty("downloadStarted")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.DownloadStarted.verify(message.downloadStarted);
                    if (error)
                        return "downloadStarted." + error;
                }
            }
            if (message.progress != null && message.hasOwnProperty("progress")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.DownloadProgress.verify(message.progress);
                    if (error)
                        return "progress." + error;
                }
            }
            if (message.segmentCompleted != null && message.hasOwnProperty("segmentCompleted")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.SegmentCompleted.verify(message.segmentCompleted);
                    if (error)
                        return "segmentCompleted." + error;
                }
            }
            if (message.downloadCompleted != null && message.hasOwnProperty("downloadCompleted")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.DownloadCompleted.verify(message.downloadCompleted);
                    if (error)
                        return "downloadCompleted." + error;
                }
            }
            if (message.downloadFailed != null && message.hasOwnProperty("downloadFailed")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.DownloadFailed.verify(message.downloadFailed);
                    if (error)
                        return "downloadFailed." + error;
                }
            }
            if (message.downloadCancelled != null && message.hasOwnProperty("downloadCancelled")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.DownloadCancelled.verify(message.downloadCancelled);
                    if (error)
                        return "downloadCancelled." + error;
                }
            }
            if (message.error != null && message.hasOwnProperty("error")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.ErrorPayload.verify(message.error);
                    if (error)
                        return "error." + error;
                }
            }
            if (message.downloadRejected != null && message.hasOwnProperty("downloadRejected")) {
                if (properties.payload === 1)
                    return "payload: multiple values";
                properties.payload = 1;
                {
                    let error = $root.download_progress.DownloadRejected.verify(message.downloadRejected);
                    if (error)
                        return "downloadRejected." + error;
                }
            }
            return null;
        };

        /**
         * Creates a WsMessage message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.WsMessage
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.WsMessage} WsMessage
         */
        WsMessage.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.WsMessage)
                return object;
            let message = new $root.download_progress.WsMessage();
            switch (object.eventType) {
            default:
                if (typeof object.eventType === "number") {
                    message.eventType = object.eventType;
                    break;
                }
                break;
            case "EVENT_TYPE_UNSPECIFIED":
            case 0:
                message.eventType = 0;
                break;
            case "EVENT_TYPE_SNAPSHOT":
            case 1:
                message.eventType = 1;
                break;
            case "EVENT_TYPE_DOWNLOAD_STARTED":
            case 2:
                message.eventType = 2;
                break;
            case "EVENT_TYPE_PROGRESS":
            case 3:
                message.eventType = 3;
                break;
            case "EVENT_TYPE_SEGMENT_COMPLETED":
            case 4:
                message.eventType = 4;
                break;
            case "EVENT_TYPE_DOWNLOAD_COMPLETED":
            case 5:
                message.eventType = 5;
                break;
            case "EVENT_TYPE_DOWNLOAD_FAILED":
            case 6:
                message.eventType = 6;
                break;
            case "EVENT_TYPE_DOWNLOAD_CANCELLED":
            case 7:
                message.eventType = 7;
                break;
            case "EVENT_TYPE_ERROR":
            case 8:
                message.eventType = 8;
                break;
            case "EVENT_TYPE_DOWNLOAD_REJECTED":
            case 9:
                message.eventType = 9;
                break;
            }
            if (object.snapshot != null) {
                if (typeof object.snapshot !== "object")
                    throw TypeError(".download_progress.WsMessage.snapshot: object expected");
                message.snapshot = $root.download_progress.DownloadSnapshot.fromObject(object.snapshot);
            }
            if (object.downloadStarted != null) {
                if (typeof object.downloadStarted !== "object")
                    throw TypeError(".download_progress.WsMessage.downloadStarted: object expected");
                message.downloadStarted = $root.download_progress.DownloadStarted.fromObject(object.downloadStarted);
            }
            if (object.progress != null) {
                if (typeof object.progress !== "object")
                    throw TypeError(".download_progress.WsMessage.progress: object expected");
                message.progress = $root.download_progress.DownloadProgress.fromObject(object.progress);
            }
            if (object.segmentCompleted != null) {
                if (typeof object.segmentCompleted !== "object")
                    throw TypeError(".download_progress.WsMessage.segmentCompleted: object expected");
                message.segmentCompleted = $root.download_progress.SegmentCompleted.fromObject(object.segmentCompleted);
            }
            if (object.downloadCompleted != null) {
                if (typeof object.downloadCompleted !== "object")
                    throw TypeError(".download_progress.WsMessage.downloadCompleted: object expected");
                message.downloadCompleted = $root.download_progress.DownloadCompleted.fromObject(object.downloadCompleted);
            }
            if (object.downloadFailed != null) {
                if (typeof object.downloadFailed !== "object")
                    throw TypeError(".download_progress.WsMessage.downloadFailed: object expected");
                message.downloadFailed = $root.download_progress.DownloadFailed.fromObject(object.downloadFailed);
            }
            if (object.downloadCancelled != null) {
                if (typeof object.downloadCancelled !== "object")
                    throw TypeError(".download_progress.WsMessage.downloadCancelled: object expected");
                message.downloadCancelled = $root.download_progress.DownloadCancelled.fromObject(object.downloadCancelled);
            }
            if (object.error != null) {
                if (typeof object.error !== "object")
                    throw TypeError(".download_progress.WsMessage.error: object expected");
                message.error = $root.download_progress.ErrorPayload.fromObject(object.error);
            }
            if (object.downloadRejected != null) {
                if (typeof object.downloadRejected !== "object")
                    throw TypeError(".download_progress.WsMessage.downloadRejected: object expected");
                message.downloadRejected = $root.download_progress.DownloadRejected.fromObject(object.downloadRejected);
            }
            return message;
        };

        /**
         * Creates a plain object from a WsMessage message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.WsMessage
         * @static
         * @param {download_progress.WsMessage} message WsMessage
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        WsMessage.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults)
                object.eventType = options.enums === String ? "EVENT_TYPE_UNSPECIFIED" : 0;
            if (message.eventType != null && message.hasOwnProperty("eventType"))
                object.eventType = options.enums === String ? $root.download_progress.EventType[message.eventType] === undefined ? message.eventType : $root.download_progress.EventType[message.eventType] : message.eventType;
            if (message.snapshot != null && message.hasOwnProperty("snapshot")) {
                object.snapshot = $root.download_progress.DownloadSnapshot.toObject(message.snapshot, options);
                if (options.oneofs)
                    object.payload = "snapshot";
            }
            if (message.downloadStarted != null && message.hasOwnProperty("downloadStarted")) {
                object.downloadStarted = $root.download_progress.DownloadStarted.toObject(message.downloadStarted, options);
                if (options.oneofs)
                    object.payload = "downloadStarted";
            }
            if (message.progress != null && message.hasOwnProperty("progress")) {
                object.progress = $root.download_progress.DownloadProgress.toObject(message.progress, options);
                if (options.oneofs)
                    object.payload = "progress";
            }
            if (message.segmentCompleted != null && message.hasOwnProperty("segmentCompleted")) {
                object.segmentCompleted = $root.download_progress.SegmentCompleted.toObject(message.segmentCompleted, options);
                if (options.oneofs)
                    object.payload = "segmentCompleted";
            }
            if (message.downloadCompleted != null && message.hasOwnProperty("downloadCompleted")) {
                object.downloadCompleted = $root.download_progress.DownloadCompleted.toObject(message.downloadCompleted, options);
                if (options.oneofs)
                    object.payload = "downloadCompleted";
            }
            if (message.downloadFailed != null && message.hasOwnProperty("downloadFailed")) {
                object.downloadFailed = $root.download_progress.DownloadFailed.toObject(message.downloadFailed, options);
                if (options.oneofs)
                    object.payload = "downloadFailed";
            }
            if (message.downloadCancelled != null && message.hasOwnProperty("downloadCancelled")) {
                object.downloadCancelled = $root.download_progress.DownloadCancelled.toObject(message.downloadCancelled, options);
                if (options.oneofs)
                    object.payload = "downloadCancelled";
            }
            if (message.error != null && message.hasOwnProperty("error")) {
                object.error = $root.download_progress.ErrorPayload.toObject(message.error, options);
                if (options.oneofs)
                    object.payload = "error";
            }
            if (message.downloadRejected != null && message.hasOwnProperty("downloadRejected")) {
                object.downloadRejected = $root.download_progress.DownloadRejected.toObject(message.downloadRejected, options);
                if (options.oneofs)
                    object.payload = "downloadRejected";
            }
            return object;
        };

        /**
         * Converts this WsMessage to JSON.
         * @function toJSON
         * @memberof download_progress.WsMessage
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        WsMessage.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for WsMessage
         * @function getTypeUrl
         * @memberof download_progress.WsMessage
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        WsMessage.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.WsMessage";
        };

        return WsMessage;
    })();

    download_progress.ClientMessage = (function() {

        /**
         * Properties of a ClientMessage.
         * @memberof download_progress
         * @interface IClientMessage
         * @property {download_progress.ISubscribeRequest|null} [subscribe] ClientMessage subscribe
         * @property {download_progress.IUnsubscribeRequest|null} [unsubscribe] ClientMessage unsubscribe
         */

        /**
         * Constructs a new ClientMessage.
         * @memberof download_progress
         * @classdesc Represents a ClientMessage.
         * @implements IClientMessage
         * @constructor
         * @param {download_progress.IClientMessage=} [properties] Properties to set
         */
        function ClientMessage(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * ClientMessage subscribe.
         * @member {download_progress.ISubscribeRequest|null|undefined} subscribe
         * @memberof download_progress.ClientMessage
         * @instance
         */
        ClientMessage.prototype.subscribe = null;

        /**
         * ClientMessage unsubscribe.
         * @member {download_progress.IUnsubscribeRequest|null|undefined} unsubscribe
         * @memberof download_progress.ClientMessage
         * @instance
         */
        ClientMessage.prototype.unsubscribe = null;

        // OneOf field names bound to virtual getters and setters
        let $oneOfFields;

        /**
         * ClientMessage action.
         * @member {"subscribe"|"unsubscribe"|undefined} action
         * @memberof download_progress.ClientMessage
         * @instance
         */
        Object.defineProperty(ClientMessage.prototype, "action", {
            get: $util.oneOfGetter($oneOfFields = ["subscribe", "unsubscribe"]),
            set: $util.oneOfSetter($oneOfFields)
        });

        /**
         * Creates a new ClientMessage instance using the specified properties.
         * @function create
         * @memberof download_progress.ClientMessage
         * @static
         * @param {download_progress.IClientMessage=} [properties] Properties to set
         * @returns {download_progress.ClientMessage} ClientMessage instance
         */
        ClientMessage.create = function create(properties) {
            return new ClientMessage(properties);
        };

        /**
         * Encodes the specified ClientMessage message. Does not implicitly {@link download_progress.ClientMessage.verify|verify} messages.
         * @function encode
         * @memberof download_progress.ClientMessage
         * @static
         * @param {download_progress.IClientMessage} message ClientMessage message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        ClientMessage.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.subscribe != null && Object.hasOwnProperty.call(message, "subscribe"))
                $root.download_progress.SubscribeRequest.encode(message.subscribe, writer.uint32(/* id 1, wireType 2 =*/10).fork()).ldelim();
            if (message.unsubscribe != null && Object.hasOwnProperty.call(message, "unsubscribe"))
                $root.download_progress.UnsubscribeRequest.encode(message.unsubscribe, writer.uint32(/* id 2, wireType 2 =*/18).fork()).ldelim();
            return writer;
        };

        /**
         * Encodes the specified ClientMessage message, length delimited. Does not implicitly {@link download_progress.ClientMessage.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.ClientMessage
         * @static
         * @param {download_progress.IClientMessage} message ClientMessage message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        ClientMessage.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a ClientMessage message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.ClientMessage
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.ClientMessage} ClientMessage
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        ClientMessage.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.ClientMessage();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.subscribe = $root.download_progress.SubscribeRequest.decode(reader, reader.uint32());
                        break;
                    }
                case 2: {
                        message.unsubscribe = $root.download_progress.UnsubscribeRequest.decode(reader, reader.uint32());
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a ClientMessage message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.ClientMessage
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.ClientMessage} ClientMessage
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        ClientMessage.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a ClientMessage message.
         * @function verify
         * @memberof download_progress.ClientMessage
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        ClientMessage.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            let properties = {};
            if (message.subscribe != null && message.hasOwnProperty("subscribe")) {
                properties.action = 1;
                {
                    let error = $root.download_progress.SubscribeRequest.verify(message.subscribe);
                    if (error)
                        return "subscribe." + error;
                }
            }
            if (message.unsubscribe != null && message.hasOwnProperty("unsubscribe")) {
                if (properties.action === 1)
                    return "action: multiple values";
                properties.action = 1;
                {
                    let error = $root.download_progress.UnsubscribeRequest.verify(message.unsubscribe);
                    if (error)
                        return "unsubscribe." + error;
                }
            }
            return null;
        };

        /**
         * Creates a ClientMessage message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.ClientMessage
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.ClientMessage} ClientMessage
         */
        ClientMessage.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.ClientMessage)
                return object;
            let message = new $root.download_progress.ClientMessage();
            if (object.subscribe != null) {
                if (typeof object.subscribe !== "object")
                    throw TypeError(".download_progress.ClientMessage.subscribe: object expected");
                message.subscribe = $root.download_progress.SubscribeRequest.fromObject(object.subscribe);
            }
            if (object.unsubscribe != null) {
                if (typeof object.unsubscribe !== "object")
                    throw TypeError(".download_progress.ClientMessage.unsubscribe: object expected");
                message.unsubscribe = $root.download_progress.UnsubscribeRequest.fromObject(object.unsubscribe);
            }
            return message;
        };

        /**
         * Creates a plain object from a ClientMessage message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.ClientMessage
         * @static
         * @param {download_progress.ClientMessage} message ClientMessage
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        ClientMessage.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (message.subscribe != null && message.hasOwnProperty("subscribe")) {
                object.subscribe = $root.download_progress.SubscribeRequest.toObject(message.subscribe, options);
                if (options.oneofs)
                    object.action = "subscribe";
            }
            if (message.unsubscribe != null && message.hasOwnProperty("unsubscribe")) {
                object.unsubscribe = $root.download_progress.UnsubscribeRequest.toObject(message.unsubscribe, options);
                if (options.oneofs)
                    object.action = "unsubscribe";
            }
            return object;
        };

        /**
         * Converts this ClientMessage to JSON.
         * @function toJSON
         * @memberof download_progress.ClientMessage
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        ClientMessage.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for ClientMessage
         * @function getTypeUrl
         * @memberof download_progress.ClientMessage
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        ClientMessage.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.ClientMessage";
        };

        return ClientMessage;
    })();

    download_progress.SubscribeRequest = (function() {

        /**
         * Properties of a SubscribeRequest.
         * @memberof download_progress
         * @interface ISubscribeRequest
         * @property {string|null} [streamerId] SubscribeRequest streamerId
         */

        /**
         * Constructs a new SubscribeRequest.
         * @memberof download_progress
         * @classdesc Represents a SubscribeRequest.
         * @implements ISubscribeRequest
         * @constructor
         * @param {download_progress.ISubscribeRequest=} [properties] Properties to set
         */
        function SubscribeRequest(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * SubscribeRequest streamerId.
         * @member {string} streamerId
         * @memberof download_progress.SubscribeRequest
         * @instance
         */
        SubscribeRequest.prototype.streamerId = "";

        /**
         * Creates a new SubscribeRequest instance using the specified properties.
         * @function create
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {download_progress.ISubscribeRequest=} [properties] Properties to set
         * @returns {download_progress.SubscribeRequest} SubscribeRequest instance
         */
        SubscribeRequest.create = function create(properties) {
            return new SubscribeRequest(properties);
        };

        /**
         * Encodes the specified SubscribeRequest message. Does not implicitly {@link download_progress.SubscribeRequest.verify|verify} messages.
         * @function encode
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {download_progress.ISubscribeRequest} message SubscribeRequest message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        SubscribeRequest.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.streamerId);
            return writer;
        };

        /**
         * Encodes the specified SubscribeRequest message, length delimited. Does not implicitly {@link download_progress.SubscribeRequest.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {download_progress.ISubscribeRequest} message SubscribeRequest message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        SubscribeRequest.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a SubscribeRequest message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.SubscribeRequest} SubscribeRequest
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        SubscribeRequest.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.SubscribeRequest();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.streamerId = reader.string();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a SubscribeRequest message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.SubscribeRequest} SubscribeRequest
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        SubscribeRequest.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a SubscribeRequest message.
         * @function verify
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        SubscribeRequest.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            return null;
        };

        /**
         * Creates a SubscribeRequest message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.SubscribeRequest} SubscribeRequest
         */
        SubscribeRequest.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.SubscribeRequest)
                return object;
            let message = new $root.download_progress.SubscribeRequest();
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            return message;
        };

        /**
         * Creates a plain object from a SubscribeRequest message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {download_progress.SubscribeRequest} message SubscribeRequest
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        SubscribeRequest.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults)
                object.streamerId = "";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            return object;
        };

        /**
         * Converts this SubscribeRequest to JSON.
         * @function toJSON
         * @memberof download_progress.SubscribeRequest
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        SubscribeRequest.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for SubscribeRequest
         * @function getTypeUrl
         * @memberof download_progress.SubscribeRequest
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        SubscribeRequest.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.SubscribeRequest";
        };

        return SubscribeRequest;
    })();

    download_progress.UnsubscribeRequest = (function() {

        /**
         * Properties of an UnsubscribeRequest.
         * @memberof download_progress
         * @interface IUnsubscribeRequest
         */

        /**
         * Constructs a new UnsubscribeRequest.
         * @memberof download_progress
         * @classdesc Represents an UnsubscribeRequest.
         * @implements IUnsubscribeRequest
         * @constructor
         * @param {download_progress.IUnsubscribeRequest=} [properties] Properties to set
         */
        function UnsubscribeRequest(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * Creates a new UnsubscribeRequest instance using the specified properties.
         * @function create
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {download_progress.IUnsubscribeRequest=} [properties] Properties to set
         * @returns {download_progress.UnsubscribeRequest} UnsubscribeRequest instance
         */
        UnsubscribeRequest.create = function create(properties) {
            return new UnsubscribeRequest(properties);
        };

        /**
         * Encodes the specified UnsubscribeRequest message. Does not implicitly {@link download_progress.UnsubscribeRequest.verify|verify} messages.
         * @function encode
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {download_progress.IUnsubscribeRequest} message UnsubscribeRequest message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        UnsubscribeRequest.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            return writer;
        };

        /**
         * Encodes the specified UnsubscribeRequest message, length delimited. Does not implicitly {@link download_progress.UnsubscribeRequest.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {download_progress.IUnsubscribeRequest} message UnsubscribeRequest message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        UnsubscribeRequest.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes an UnsubscribeRequest message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.UnsubscribeRequest} UnsubscribeRequest
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        UnsubscribeRequest.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.UnsubscribeRequest();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes an UnsubscribeRequest message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.UnsubscribeRequest} UnsubscribeRequest
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        UnsubscribeRequest.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies an UnsubscribeRequest message.
         * @function verify
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        UnsubscribeRequest.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            return null;
        };

        /**
         * Creates an UnsubscribeRequest message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.UnsubscribeRequest} UnsubscribeRequest
         */
        UnsubscribeRequest.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.UnsubscribeRequest)
                return object;
            return new $root.download_progress.UnsubscribeRequest();
        };

        /**
         * Creates a plain object from an UnsubscribeRequest message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {download_progress.UnsubscribeRequest} message UnsubscribeRequest
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        UnsubscribeRequest.toObject = function toObject() {
            return {};
        };

        /**
         * Converts this UnsubscribeRequest to JSON.
         * @function toJSON
         * @memberof download_progress.UnsubscribeRequest
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        UnsubscribeRequest.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for UnsubscribeRequest
         * @function getTypeUrl
         * @memberof download_progress.UnsubscribeRequest
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        UnsubscribeRequest.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.UnsubscribeRequest";
        };

        return UnsubscribeRequest;
    })();

    download_progress.DownloadSnapshot = (function() {

        /**
         * Properties of a DownloadSnapshot.
         * @memberof download_progress
         * @interface IDownloadSnapshot
         * @property {Array.<download_progress.IDownloadProgress>|null} [downloads] DownloadSnapshot downloads
         */

        /**
         * Constructs a new DownloadSnapshot.
         * @memberof download_progress
         * @classdesc Represents a DownloadSnapshot.
         * @implements IDownloadSnapshot
         * @constructor
         * @param {download_progress.IDownloadSnapshot=} [properties] Properties to set
         */
        function DownloadSnapshot(properties) {
            this.downloads = [];
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * DownloadSnapshot downloads.
         * @member {Array.<download_progress.IDownloadProgress>} downloads
         * @memberof download_progress.DownloadSnapshot
         * @instance
         */
        DownloadSnapshot.prototype.downloads = $util.emptyArray;

        /**
         * Creates a new DownloadSnapshot instance using the specified properties.
         * @function create
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {download_progress.IDownloadSnapshot=} [properties] Properties to set
         * @returns {download_progress.DownloadSnapshot} DownloadSnapshot instance
         */
        DownloadSnapshot.create = function create(properties) {
            return new DownloadSnapshot(properties);
        };

        /**
         * Encodes the specified DownloadSnapshot message. Does not implicitly {@link download_progress.DownloadSnapshot.verify|verify} messages.
         * @function encode
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {download_progress.IDownloadSnapshot} message DownloadSnapshot message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadSnapshot.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.downloads != null && message.downloads.length)
                for (let i = 0; i < message.downloads.length; ++i)
                    $root.download_progress.DownloadProgress.encode(message.downloads[i], writer.uint32(/* id 1, wireType 2 =*/10).fork()).ldelim();
            return writer;
        };

        /**
         * Encodes the specified DownloadSnapshot message, length delimited. Does not implicitly {@link download_progress.DownloadSnapshot.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {download_progress.IDownloadSnapshot} message DownloadSnapshot message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadSnapshot.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a DownloadSnapshot message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.DownloadSnapshot} DownloadSnapshot
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadSnapshot.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.DownloadSnapshot();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        if (!(message.downloads && message.downloads.length))
                            message.downloads = [];
                        message.downloads.push($root.download_progress.DownloadProgress.decode(reader, reader.uint32()));
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a DownloadSnapshot message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.DownloadSnapshot} DownloadSnapshot
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadSnapshot.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a DownloadSnapshot message.
         * @function verify
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        DownloadSnapshot.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.downloads != null && message.hasOwnProperty("downloads")) {
                if (!Array.isArray(message.downloads))
                    return "downloads: array expected";
                for (let i = 0; i < message.downloads.length; ++i) {
                    let error = $root.download_progress.DownloadProgress.verify(message.downloads[i]);
                    if (error)
                        return "downloads." + error;
                }
            }
            return null;
        };

        /**
         * Creates a DownloadSnapshot message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.DownloadSnapshot} DownloadSnapshot
         */
        DownloadSnapshot.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.DownloadSnapshot)
                return object;
            let message = new $root.download_progress.DownloadSnapshot();
            if (object.downloads) {
                if (!Array.isArray(object.downloads))
                    throw TypeError(".download_progress.DownloadSnapshot.downloads: array expected");
                message.downloads = [];
                for (let i = 0; i < object.downloads.length; ++i) {
                    if (typeof object.downloads[i] !== "object")
                        throw TypeError(".download_progress.DownloadSnapshot.downloads: object expected");
                    message.downloads[i] = $root.download_progress.DownloadProgress.fromObject(object.downloads[i]);
                }
            }
            return message;
        };

        /**
         * Creates a plain object from a DownloadSnapshot message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {download_progress.DownloadSnapshot} message DownloadSnapshot
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        DownloadSnapshot.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.arrays || options.defaults)
                object.downloads = [];
            if (message.downloads && message.downloads.length) {
                object.downloads = [];
                for (let j = 0; j < message.downloads.length; ++j)
                    object.downloads[j] = $root.download_progress.DownloadProgress.toObject(message.downloads[j], options);
            }
            return object;
        };

        /**
         * Converts this DownloadSnapshot to JSON.
         * @function toJSON
         * @memberof download_progress.DownloadSnapshot
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        DownloadSnapshot.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for DownloadSnapshot
         * @function getTypeUrl
         * @memberof download_progress.DownloadSnapshot
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        DownloadSnapshot.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.DownloadSnapshot";
        };

        return DownloadSnapshot;
    })();

    download_progress.DownloadProgress = (function() {

        /**
         * Properties of a DownloadProgress.
         * @memberof download_progress
         * @interface IDownloadProgress
         * @property {string|null} [downloadId] DownloadProgress downloadId
         * @property {string|null} [streamerId] DownloadProgress streamerId
         * @property {string|null} [sessionId] DownloadProgress sessionId
         * @property {string|null} [engineType] DownloadProgress engineType
         * @property {string|null} [status] DownloadProgress status
         * @property {number|Long|null} [bytesDownloaded] DownloadProgress bytesDownloaded
         * @property {number|null} [durationSecs] DownloadProgress durationSecs
         * @property {number|Long|null} [speedBytesPerSec] DownloadProgress speedBytesPerSec
         * @property {number|null} [segmentsCompleted] DownloadProgress segmentsCompleted
         * @property {number|null} [mediaDurationSecs] DownloadProgress mediaDurationSecs
         * @property {number|null} [playbackRatio] DownloadProgress playbackRatio
         * @property {number|Long|null} [startedAtMs] DownloadProgress startedAtMs
         * @property {string|null} [downloadUrl] DownloadProgress downloadUrl
         */

        /**
         * Constructs a new DownloadProgress.
         * @memberof download_progress
         * @classdesc Represents a DownloadProgress.
         * @implements IDownloadProgress
         * @constructor
         * @param {download_progress.IDownloadProgress=} [properties] Properties to set
         */
        function DownloadProgress(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * DownloadProgress downloadId.
         * @member {string} downloadId
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.downloadId = "";

        /**
         * DownloadProgress streamerId.
         * @member {string} streamerId
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.streamerId = "";

        /**
         * DownloadProgress sessionId.
         * @member {string} sessionId
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.sessionId = "";

        /**
         * DownloadProgress engineType.
         * @member {string} engineType
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.engineType = "";

        /**
         * DownloadProgress status.
         * @member {string} status
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.status = "";

        /**
         * DownloadProgress bytesDownloaded.
         * @member {number|Long} bytesDownloaded
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.bytesDownloaded = $util.Long ? $util.Long.fromBits(0,0,true) : 0;

        /**
         * DownloadProgress durationSecs.
         * @member {number} durationSecs
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.durationSecs = 0;

        /**
         * DownloadProgress speedBytesPerSec.
         * @member {number|Long} speedBytesPerSec
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.speedBytesPerSec = $util.Long ? $util.Long.fromBits(0,0,true) : 0;

        /**
         * DownloadProgress segmentsCompleted.
         * @member {number} segmentsCompleted
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.segmentsCompleted = 0;

        /**
         * DownloadProgress mediaDurationSecs.
         * @member {number} mediaDurationSecs
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.mediaDurationSecs = 0;

        /**
         * DownloadProgress playbackRatio.
         * @member {number} playbackRatio
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.playbackRatio = 0;

        /**
         * DownloadProgress startedAtMs.
         * @member {number|Long} startedAtMs
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.startedAtMs = $util.Long ? $util.Long.fromBits(0,0,false) : 0;

        /**
         * DownloadProgress downloadUrl.
         * @member {string} downloadUrl
         * @memberof download_progress.DownloadProgress
         * @instance
         */
        DownloadProgress.prototype.downloadUrl = "";

        /**
         * Creates a new DownloadProgress instance using the specified properties.
         * @function create
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {download_progress.IDownloadProgress=} [properties] Properties to set
         * @returns {download_progress.DownloadProgress} DownloadProgress instance
         */
        DownloadProgress.create = function create(properties) {
            return new DownloadProgress(properties);
        };

        /**
         * Encodes the specified DownloadProgress message. Does not implicitly {@link download_progress.DownloadProgress.verify|verify} messages.
         * @function encode
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {download_progress.IDownloadProgress} message DownloadProgress message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadProgress.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.downloadId != null && Object.hasOwnProperty.call(message, "downloadId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.downloadId);
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.streamerId);
            if (message.sessionId != null && Object.hasOwnProperty.call(message, "sessionId"))
                writer.uint32(/* id 3, wireType 2 =*/26).string(message.sessionId);
            if (message.engineType != null && Object.hasOwnProperty.call(message, "engineType"))
                writer.uint32(/* id 4, wireType 2 =*/34).string(message.engineType);
            if (message.status != null && Object.hasOwnProperty.call(message, "status"))
                writer.uint32(/* id 5, wireType 2 =*/42).string(message.status);
            if (message.bytesDownloaded != null && Object.hasOwnProperty.call(message, "bytesDownloaded"))
                writer.uint32(/* id 6, wireType 0 =*/48).uint64(message.bytesDownloaded);
            if (message.durationSecs != null && Object.hasOwnProperty.call(message, "durationSecs"))
                writer.uint32(/* id 7, wireType 1 =*/57).double(message.durationSecs);
            if (message.speedBytesPerSec != null && Object.hasOwnProperty.call(message, "speedBytesPerSec"))
                writer.uint32(/* id 8, wireType 0 =*/64).uint64(message.speedBytesPerSec);
            if (message.segmentsCompleted != null && Object.hasOwnProperty.call(message, "segmentsCompleted"))
                writer.uint32(/* id 9, wireType 0 =*/72).uint32(message.segmentsCompleted);
            if (message.mediaDurationSecs != null && Object.hasOwnProperty.call(message, "mediaDurationSecs"))
                writer.uint32(/* id 10, wireType 1 =*/81).double(message.mediaDurationSecs);
            if (message.playbackRatio != null && Object.hasOwnProperty.call(message, "playbackRatio"))
                writer.uint32(/* id 11, wireType 1 =*/89).double(message.playbackRatio);
            if (message.startedAtMs != null && Object.hasOwnProperty.call(message, "startedAtMs"))
                writer.uint32(/* id 12, wireType 0 =*/96).int64(message.startedAtMs);
            if (message.downloadUrl != null && Object.hasOwnProperty.call(message, "downloadUrl"))
                writer.uint32(/* id 13, wireType 2 =*/106).string(message.downloadUrl);
            return writer;
        };

        /**
         * Encodes the specified DownloadProgress message, length delimited. Does not implicitly {@link download_progress.DownloadProgress.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {download_progress.IDownloadProgress} message DownloadProgress message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadProgress.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a DownloadProgress message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.DownloadProgress} DownloadProgress
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadProgress.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.DownloadProgress();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.downloadId = reader.string();
                        break;
                    }
                case 2: {
                        message.streamerId = reader.string();
                        break;
                    }
                case 3: {
                        message.sessionId = reader.string();
                        break;
                    }
                case 4: {
                        message.engineType = reader.string();
                        break;
                    }
                case 5: {
                        message.status = reader.string();
                        break;
                    }
                case 6: {
                        message.bytesDownloaded = reader.uint64();
                        break;
                    }
                case 7: {
                        message.durationSecs = reader.double();
                        break;
                    }
                case 8: {
                        message.speedBytesPerSec = reader.uint64();
                        break;
                    }
                case 9: {
                        message.segmentsCompleted = reader.uint32();
                        break;
                    }
                case 10: {
                        message.mediaDurationSecs = reader.double();
                        break;
                    }
                case 11: {
                        message.playbackRatio = reader.double();
                        break;
                    }
                case 12: {
                        message.startedAtMs = reader.int64();
                        break;
                    }
                case 13: {
                        message.downloadUrl = reader.string();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a DownloadProgress message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.DownloadProgress} DownloadProgress
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadProgress.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a DownloadProgress message.
         * @function verify
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        DownloadProgress.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                if (!$util.isString(message.downloadId))
                    return "downloadId: string expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                if (!$util.isString(message.sessionId))
                    return "sessionId: string expected";
            if (message.engineType != null && message.hasOwnProperty("engineType"))
                if (!$util.isString(message.engineType))
                    return "engineType: string expected";
            if (message.status != null && message.hasOwnProperty("status"))
                if (!$util.isString(message.status))
                    return "status: string expected";
            if (message.bytesDownloaded != null && message.hasOwnProperty("bytesDownloaded"))
                if (!$util.isInteger(message.bytesDownloaded) && !(message.bytesDownloaded && $util.isInteger(message.bytesDownloaded.low) && $util.isInteger(message.bytesDownloaded.high)))
                    return "bytesDownloaded: integer|Long expected";
            if (message.durationSecs != null && message.hasOwnProperty("durationSecs"))
                if (typeof message.durationSecs !== "number")
                    return "durationSecs: number expected";
            if (message.speedBytesPerSec != null && message.hasOwnProperty("speedBytesPerSec"))
                if (!$util.isInteger(message.speedBytesPerSec) && !(message.speedBytesPerSec && $util.isInteger(message.speedBytesPerSec.low) && $util.isInteger(message.speedBytesPerSec.high)))
                    return "speedBytesPerSec: integer|Long expected";
            if (message.segmentsCompleted != null && message.hasOwnProperty("segmentsCompleted"))
                if (!$util.isInteger(message.segmentsCompleted))
                    return "segmentsCompleted: integer expected";
            if (message.mediaDurationSecs != null && message.hasOwnProperty("mediaDurationSecs"))
                if (typeof message.mediaDurationSecs !== "number")
                    return "mediaDurationSecs: number expected";
            if (message.playbackRatio != null && message.hasOwnProperty("playbackRatio"))
                if (typeof message.playbackRatio !== "number")
                    return "playbackRatio: number expected";
            if (message.startedAtMs != null && message.hasOwnProperty("startedAtMs"))
                if (!$util.isInteger(message.startedAtMs) && !(message.startedAtMs && $util.isInteger(message.startedAtMs.low) && $util.isInteger(message.startedAtMs.high)))
                    return "startedAtMs: integer|Long expected";
            if (message.downloadUrl != null && message.hasOwnProperty("downloadUrl"))
                if (!$util.isString(message.downloadUrl))
                    return "downloadUrl: string expected";
            return null;
        };

        /**
         * Creates a DownloadProgress message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.DownloadProgress} DownloadProgress
         */
        DownloadProgress.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.DownloadProgress)
                return object;
            let message = new $root.download_progress.DownloadProgress();
            if (object.downloadId != null)
                message.downloadId = String(object.downloadId);
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            if (object.sessionId != null)
                message.sessionId = String(object.sessionId);
            if (object.engineType != null)
                message.engineType = String(object.engineType);
            if (object.status != null)
                message.status = String(object.status);
            if (object.bytesDownloaded != null)
                if ($util.Long)
                    (message.bytesDownloaded = $util.Long.fromValue(object.bytesDownloaded)).unsigned = true;
                else if (typeof object.bytesDownloaded === "string")
                    message.bytesDownloaded = parseInt(object.bytesDownloaded, 10);
                else if (typeof object.bytesDownloaded === "number")
                    message.bytesDownloaded = object.bytesDownloaded;
                else if (typeof object.bytesDownloaded === "object")
                    message.bytesDownloaded = new $util.LongBits(object.bytesDownloaded.low >>> 0, object.bytesDownloaded.high >>> 0).toNumber(true);
            if (object.durationSecs != null)
                message.durationSecs = Number(object.durationSecs);
            if (object.speedBytesPerSec != null)
                if ($util.Long)
                    (message.speedBytesPerSec = $util.Long.fromValue(object.speedBytesPerSec)).unsigned = true;
                else if (typeof object.speedBytesPerSec === "string")
                    message.speedBytesPerSec = parseInt(object.speedBytesPerSec, 10);
                else if (typeof object.speedBytesPerSec === "number")
                    message.speedBytesPerSec = object.speedBytesPerSec;
                else if (typeof object.speedBytesPerSec === "object")
                    message.speedBytesPerSec = new $util.LongBits(object.speedBytesPerSec.low >>> 0, object.speedBytesPerSec.high >>> 0).toNumber(true);
            if (object.segmentsCompleted != null)
                message.segmentsCompleted = object.segmentsCompleted >>> 0;
            if (object.mediaDurationSecs != null)
                message.mediaDurationSecs = Number(object.mediaDurationSecs);
            if (object.playbackRatio != null)
                message.playbackRatio = Number(object.playbackRatio);
            if (object.startedAtMs != null)
                if ($util.Long)
                    (message.startedAtMs = $util.Long.fromValue(object.startedAtMs)).unsigned = false;
                else if (typeof object.startedAtMs === "string")
                    message.startedAtMs = parseInt(object.startedAtMs, 10);
                else if (typeof object.startedAtMs === "number")
                    message.startedAtMs = object.startedAtMs;
                else if (typeof object.startedAtMs === "object")
                    message.startedAtMs = new $util.LongBits(object.startedAtMs.low >>> 0, object.startedAtMs.high >>> 0).toNumber();
            if (object.downloadUrl != null)
                message.downloadUrl = String(object.downloadUrl);
            return message;
        };

        /**
         * Creates a plain object from a DownloadProgress message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {download_progress.DownloadProgress} message DownloadProgress
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        DownloadProgress.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.downloadId = "";
                object.streamerId = "";
                object.sessionId = "";
                object.engineType = "";
                object.status = "";
                if ($util.Long) {
                    let long = new $util.Long(0, 0, true);
                    object.bytesDownloaded = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                } else
                    object.bytesDownloaded = options.longs === String ? "0" : 0;
                object.durationSecs = 0;
                if ($util.Long) {
                    let long = new $util.Long(0, 0, true);
                    object.speedBytesPerSec = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                } else
                    object.speedBytesPerSec = options.longs === String ? "0" : 0;
                object.segmentsCompleted = 0;
                object.mediaDurationSecs = 0;
                object.playbackRatio = 0;
                if ($util.Long) {
                    let long = new $util.Long(0, 0, false);
                    object.startedAtMs = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                } else
                    object.startedAtMs = options.longs === String ? "0" : 0;
                object.downloadUrl = "";
            }
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                object.downloadId = message.downloadId;
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                object.sessionId = message.sessionId;
            if (message.engineType != null && message.hasOwnProperty("engineType"))
                object.engineType = message.engineType;
            if (message.status != null && message.hasOwnProperty("status"))
                object.status = message.status;
            if (message.bytesDownloaded != null && message.hasOwnProperty("bytesDownloaded"))
                if (typeof message.bytesDownloaded === "number")
                    object.bytesDownloaded = options.longs === String ? String(message.bytesDownloaded) : message.bytesDownloaded;
                else
                    object.bytesDownloaded = options.longs === String ? $util.Long.prototype.toString.call(message.bytesDownloaded) : options.longs === Number ? new $util.LongBits(message.bytesDownloaded.low >>> 0, message.bytesDownloaded.high >>> 0).toNumber(true) : message.bytesDownloaded;
            if (message.durationSecs != null && message.hasOwnProperty("durationSecs"))
                object.durationSecs = options.json && !isFinite(message.durationSecs) ? String(message.durationSecs) : message.durationSecs;
            if (message.speedBytesPerSec != null && message.hasOwnProperty("speedBytesPerSec"))
                if (typeof message.speedBytesPerSec === "number")
                    object.speedBytesPerSec = options.longs === String ? String(message.speedBytesPerSec) : message.speedBytesPerSec;
                else
                    object.speedBytesPerSec = options.longs === String ? $util.Long.prototype.toString.call(message.speedBytesPerSec) : options.longs === Number ? new $util.LongBits(message.speedBytesPerSec.low >>> 0, message.speedBytesPerSec.high >>> 0).toNumber(true) : message.speedBytesPerSec;
            if (message.segmentsCompleted != null && message.hasOwnProperty("segmentsCompleted"))
                object.segmentsCompleted = message.segmentsCompleted;
            if (message.mediaDurationSecs != null && message.hasOwnProperty("mediaDurationSecs"))
                object.mediaDurationSecs = options.json && !isFinite(message.mediaDurationSecs) ? String(message.mediaDurationSecs) : message.mediaDurationSecs;
            if (message.playbackRatio != null && message.hasOwnProperty("playbackRatio"))
                object.playbackRatio = options.json && !isFinite(message.playbackRatio) ? String(message.playbackRatio) : message.playbackRatio;
            if (message.startedAtMs != null && message.hasOwnProperty("startedAtMs"))
                if (typeof message.startedAtMs === "number")
                    object.startedAtMs = options.longs === String ? String(message.startedAtMs) : message.startedAtMs;
                else
                    object.startedAtMs = options.longs === String ? $util.Long.prototype.toString.call(message.startedAtMs) : options.longs === Number ? new $util.LongBits(message.startedAtMs.low >>> 0, message.startedAtMs.high >>> 0).toNumber() : message.startedAtMs;
            if (message.downloadUrl != null && message.hasOwnProperty("downloadUrl"))
                object.downloadUrl = message.downloadUrl;
            return object;
        };

        /**
         * Converts this DownloadProgress to JSON.
         * @function toJSON
         * @memberof download_progress.DownloadProgress
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        DownloadProgress.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for DownloadProgress
         * @function getTypeUrl
         * @memberof download_progress.DownloadProgress
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        DownloadProgress.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.DownloadProgress";
        };

        return DownloadProgress;
    })();

    download_progress.DownloadStarted = (function() {

        /**
         * Properties of a DownloadStarted.
         * @memberof download_progress
         * @interface IDownloadStarted
         * @property {string|null} [downloadId] DownloadStarted downloadId
         * @property {string|null} [streamerId] DownloadStarted streamerId
         * @property {string|null} [sessionId] DownloadStarted sessionId
         * @property {string|null} [engineType] DownloadStarted engineType
         * @property {number|Long|null} [startedAtMs] DownloadStarted startedAtMs
         */

        /**
         * Constructs a new DownloadStarted.
         * @memberof download_progress
         * @classdesc Represents a DownloadStarted.
         * @implements IDownloadStarted
         * @constructor
         * @param {download_progress.IDownloadStarted=} [properties] Properties to set
         */
        function DownloadStarted(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * DownloadStarted downloadId.
         * @member {string} downloadId
         * @memberof download_progress.DownloadStarted
         * @instance
         */
        DownloadStarted.prototype.downloadId = "";

        /**
         * DownloadStarted streamerId.
         * @member {string} streamerId
         * @memberof download_progress.DownloadStarted
         * @instance
         */
        DownloadStarted.prototype.streamerId = "";

        /**
         * DownloadStarted sessionId.
         * @member {string} sessionId
         * @memberof download_progress.DownloadStarted
         * @instance
         */
        DownloadStarted.prototype.sessionId = "";

        /**
         * DownloadStarted engineType.
         * @member {string} engineType
         * @memberof download_progress.DownloadStarted
         * @instance
         */
        DownloadStarted.prototype.engineType = "";

        /**
         * DownloadStarted startedAtMs.
         * @member {number|Long} startedAtMs
         * @memberof download_progress.DownloadStarted
         * @instance
         */
        DownloadStarted.prototype.startedAtMs = $util.Long ? $util.Long.fromBits(0,0,false) : 0;

        /**
         * Creates a new DownloadStarted instance using the specified properties.
         * @function create
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {download_progress.IDownloadStarted=} [properties] Properties to set
         * @returns {download_progress.DownloadStarted} DownloadStarted instance
         */
        DownloadStarted.create = function create(properties) {
            return new DownloadStarted(properties);
        };

        /**
         * Encodes the specified DownloadStarted message. Does not implicitly {@link download_progress.DownloadStarted.verify|verify} messages.
         * @function encode
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {download_progress.IDownloadStarted} message DownloadStarted message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadStarted.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.downloadId != null && Object.hasOwnProperty.call(message, "downloadId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.downloadId);
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.streamerId);
            if (message.sessionId != null && Object.hasOwnProperty.call(message, "sessionId"))
                writer.uint32(/* id 3, wireType 2 =*/26).string(message.sessionId);
            if (message.engineType != null && Object.hasOwnProperty.call(message, "engineType"))
                writer.uint32(/* id 4, wireType 2 =*/34).string(message.engineType);
            if (message.startedAtMs != null && Object.hasOwnProperty.call(message, "startedAtMs"))
                writer.uint32(/* id 5, wireType 0 =*/40).int64(message.startedAtMs);
            return writer;
        };

        /**
         * Encodes the specified DownloadStarted message, length delimited. Does not implicitly {@link download_progress.DownloadStarted.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {download_progress.IDownloadStarted} message DownloadStarted message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadStarted.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a DownloadStarted message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.DownloadStarted} DownloadStarted
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadStarted.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.DownloadStarted();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.downloadId = reader.string();
                        break;
                    }
                case 2: {
                        message.streamerId = reader.string();
                        break;
                    }
                case 3: {
                        message.sessionId = reader.string();
                        break;
                    }
                case 4: {
                        message.engineType = reader.string();
                        break;
                    }
                case 5: {
                        message.startedAtMs = reader.int64();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a DownloadStarted message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.DownloadStarted} DownloadStarted
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadStarted.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a DownloadStarted message.
         * @function verify
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        DownloadStarted.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                if (!$util.isString(message.downloadId))
                    return "downloadId: string expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                if (!$util.isString(message.sessionId))
                    return "sessionId: string expected";
            if (message.engineType != null && message.hasOwnProperty("engineType"))
                if (!$util.isString(message.engineType))
                    return "engineType: string expected";
            if (message.startedAtMs != null && message.hasOwnProperty("startedAtMs"))
                if (!$util.isInteger(message.startedAtMs) && !(message.startedAtMs && $util.isInteger(message.startedAtMs.low) && $util.isInteger(message.startedAtMs.high)))
                    return "startedAtMs: integer|Long expected";
            return null;
        };

        /**
         * Creates a DownloadStarted message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.DownloadStarted} DownloadStarted
         */
        DownloadStarted.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.DownloadStarted)
                return object;
            let message = new $root.download_progress.DownloadStarted();
            if (object.downloadId != null)
                message.downloadId = String(object.downloadId);
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            if (object.sessionId != null)
                message.sessionId = String(object.sessionId);
            if (object.engineType != null)
                message.engineType = String(object.engineType);
            if (object.startedAtMs != null)
                if ($util.Long)
                    (message.startedAtMs = $util.Long.fromValue(object.startedAtMs)).unsigned = false;
                else if (typeof object.startedAtMs === "string")
                    message.startedAtMs = parseInt(object.startedAtMs, 10);
                else if (typeof object.startedAtMs === "number")
                    message.startedAtMs = object.startedAtMs;
                else if (typeof object.startedAtMs === "object")
                    message.startedAtMs = new $util.LongBits(object.startedAtMs.low >>> 0, object.startedAtMs.high >>> 0).toNumber();
            return message;
        };

        /**
         * Creates a plain object from a DownloadStarted message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {download_progress.DownloadStarted} message DownloadStarted
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        DownloadStarted.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.downloadId = "";
                object.streamerId = "";
                object.sessionId = "";
                object.engineType = "";
                if ($util.Long) {
                    let long = new $util.Long(0, 0, false);
                    object.startedAtMs = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                } else
                    object.startedAtMs = options.longs === String ? "0" : 0;
            }
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                object.downloadId = message.downloadId;
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                object.sessionId = message.sessionId;
            if (message.engineType != null && message.hasOwnProperty("engineType"))
                object.engineType = message.engineType;
            if (message.startedAtMs != null && message.hasOwnProperty("startedAtMs"))
                if (typeof message.startedAtMs === "number")
                    object.startedAtMs = options.longs === String ? String(message.startedAtMs) : message.startedAtMs;
                else
                    object.startedAtMs = options.longs === String ? $util.Long.prototype.toString.call(message.startedAtMs) : options.longs === Number ? new $util.LongBits(message.startedAtMs.low >>> 0, message.startedAtMs.high >>> 0).toNumber() : message.startedAtMs;
            return object;
        };

        /**
         * Converts this DownloadStarted to JSON.
         * @function toJSON
         * @memberof download_progress.DownloadStarted
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        DownloadStarted.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for DownloadStarted
         * @function getTypeUrl
         * @memberof download_progress.DownloadStarted
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        DownloadStarted.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.DownloadStarted";
        };

        return DownloadStarted;
    })();

    download_progress.SegmentCompleted = (function() {

        /**
         * Properties of a SegmentCompleted.
         * @memberof download_progress
         * @interface ISegmentCompleted
         * @property {string|null} [downloadId] SegmentCompleted downloadId
         * @property {string|null} [streamerId] SegmentCompleted streamerId
         * @property {string|null} [segmentPath] SegmentCompleted segmentPath
         * @property {number|null} [segmentIndex] SegmentCompleted segmentIndex
         * @property {number|null} [durationSecs] SegmentCompleted durationSecs
         * @property {number|Long|null} [sizeBytes] SegmentCompleted sizeBytes
         * @property {string|null} [sessionId] SegmentCompleted sessionId
         */

        /**
         * Constructs a new SegmentCompleted.
         * @memberof download_progress
         * @classdesc Represents a SegmentCompleted.
         * @implements ISegmentCompleted
         * @constructor
         * @param {download_progress.ISegmentCompleted=} [properties] Properties to set
         */
        function SegmentCompleted(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * SegmentCompleted downloadId.
         * @member {string} downloadId
         * @memberof download_progress.SegmentCompleted
         * @instance
         */
        SegmentCompleted.prototype.downloadId = "";

        /**
         * SegmentCompleted streamerId.
         * @member {string} streamerId
         * @memberof download_progress.SegmentCompleted
         * @instance
         */
        SegmentCompleted.prototype.streamerId = "";

        /**
         * SegmentCompleted segmentPath.
         * @member {string} segmentPath
         * @memberof download_progress.SegmentCompleted
         * @instance
         */
        SegmentCompleted.prototype.segmentPath = "";

        /**
         * SegmentCompleted segmentIndex.
         * @member {number} segmentIndex
         * @memberof download_progress.SegmentCompleted
         * @instance
         */
        SegmentCompleted.prototype.segmentIndex = 0;

        /**
         * SegmentCompleted durationSecs.
         * @member {number} durationSecs
         * @memberof download_progress.SegmentCompleted
         * @instance
         */
        SegmentCompleted.prototype.durationSecs = 0;

        /**
         * SegmentCompleted sizeBytes.
         * @member {number|Long} sizeBytes
         * @memberof download_progress.SegmentCompleted
         * @instance
         */
        SegmentCompleted.prototype.sizeBytes = $util.Long ? $util.Long.fromBits(0,0,true) : 0;

        /**
         * SegmentCompleted sessionId.
         * @member {string} sessionId
         * @memberof download_progress.SegmentCompleted
         * @instance
         */
        SegmentCompleted.prototype.sessionId = "";

        /**
         * Creates a new SegmentCompleted instance using the specified properties.
         * @function create
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {download_progress.ISegmentCompleted=} [properties] Properties to set
         * @returns {download_progress.SegmentCompleted} SegmentCompleted instance
         */
        SegmentCompleted.create = function create(properties) {
            return new SegmentCompleted(properties);
        };

        /**
         * Encodes the specified SegmentCompleted message. Does not implicitly {@link download_progress.SegmentCompleted.verify|verify} messages.
         * @function encode
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {download_progress.ISegmentCompleted} message SegmentCompleted message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        SegmentCompleted.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.downloadId != null && Object.hasOwnProperty.call(message, "downloadId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.downloadId);
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.streamerId);
            if (message.segmentPath != null && Object.hasOwnProperty.call(message, "segmentPath"))
                writer.uint32(/* id 3, wireType 2 =*/26).string(message.segmentPath);
            if (message.segmentIndex != null && Object.hasOwnProperty.call(message, "segmentIndex"))
                writer.uint32(/* id 4, wireType 0 =*/32).uint32(message.segmentIndex);
            if (message.durationSecs != null && Object.hasOwnProperty.call(message, "durationSecs"))
                writer.uint32(/* id 5, wireType 1 =*/41).double(message.durationSecs);
            if (message.sizeBytes != null && Object.hasOwnProperty.call(message, "sizeBytes"))
                writer.uint32(/* id 6, wireType 0 =*/48).uint64(message.sizeBytes);
            if (message.sessionId != null && Object.hasOwnProperty.call(message, "sessionId"))
                writer.uint32(/* id 7, wireType 2 =*/58).string(message.sessionId);
            return writer;
        };

        /**
         * Encodes the specified SegmentCompleted message, length delimited. Does not implicitly {@link download_progress.SegmentCompleted.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {download_progress.ISegmentCompleted} message SegmentCompleted message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        SegmentCompleted.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a SegmentCompleted message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.SegmentCompleted} SegmentCompleted
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        SegmentCompleted.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.SegmentCompleted();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.downloadId = reader.string();
                        break;
                    }
                case 2: {
                        message.streamerId = reader.string();
                        break;
                    }
                case 3: {
                        message.segmentPath = reader.string();
                        break;
                    }
                case 4: {
                        message.segmentIndex = reader.uint32();
                        break;
                    }
                case 5: {
                        message.durationSecs = reader.double();
                        break;
                    }
                case 6: {
                        message.sizeBytes = reader.uint64();
                        break;
                    }
                case 7: {
                        message.sessionId = reader.string();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a SegmentCompleted message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.SegmentCompleted} SegmentCompleted
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        SegmentCompleted.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a SegmentCompleted message.
         * @function verify
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        SegmentCompleted.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                if (!$util.isString(message.downloadId))
                    return "downloadId: string expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            if (message.segmentPath != null && message.hasOwnProperty("segmentPath"))
                if (!$util.isString(message.segmentPath))
                    return "segmentPath: string expected";
            if (message.segmentIndex != null && message.hasOwnProperty("segmentIndex"))
                if (!$util.isInteger(message.segmentIndex))
                    return "segmentIndex: integer expected";
            if (message.durationSecs != null && message.hasOwnProperty("durationSecs"))
                if (typeof message.durationSecs !== "number")
                    return "durationSecs: number expected";
            if (message.sizeBytes != null && message.hasOwnProperty("sizeBytes"))
                if (!$util.isInteger(message.sizeBytes) && !(message.sizeBytes && $util.isInteger(message.sizeBytes.low) && $util.isInteger(message.sizeBytes.high)))
                    return "sizeBytes: integer|Long expected";
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                if (!$util.isString(message.sessionId))
                    return "sessionId: string expected";
            return null;
        };

        /**
         * Creates a SegmentCompleted message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.SegmentCompleted} SegmentCompleted
         */
        SegmentCompleted.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.SegmentCompleted)
                return object;
            let message = new $root.download_progress.SegmentCompleted();
            if (object.downloadId != null)
                message.downloadId = String(object.downloadId);
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            if (object.segmentPath != null)
                message.segmentPath = String(object.segmentPath);
            if (object.segmentIndex != null)
                message.segmentIndex = object.segmentIndex >>> 0;
            if (object.durationSecs != null)
                message.durationSecs = Number(object.durationSecs);
            if (object.sizeBytes != null)
                if ($util.Long)
                    (message.sizeBytes = $util.Long.fromValue(object.sizeBytes)).unsigned = true;
                else if (typeof object.sizeBytes === "string")
                    message.sizeBytes = parseInt(object.sizeBytes, 10);
                else if (typeof object.sizeBytes === "number")
                    message.sizeBytes = object.sizeBytes;
                else if (typeof object.sizeBytes === "object")
                    message.sizeBytes = new $util.LongBits(object.sizeBytes.low >>> 0, object.sizeBytes.high >>> 0).toNumber(true);
            if (object.sessionId != null)
                message.sessionId = String(object.sessionId);
            return message;
        };

        /**
         * Creates a plain object from a SegmentCompleted message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {download_progress.SegmentCompleted} message SegmentCompleted
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        SegmentCompleted.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.downloadId = "";
                object.streamerId = "";
                object.segmentPath = "";
                object.segmentIndex = 0;
                object.durationSecs = 0;
                if ($util.Long) {
                    let long = new $util.Long(0, 0, true);
                    object.sizeBytes = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                } else
                    object.sizeBytes = options.longs === String ? "0" : 0;
                object.sessionId = "";
            }
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                object.downloadId = message.downloadId;
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            if (message.segmentPath != null && message.hasOwnProperty("segmentPath"))
                object.segmentPath = message.segmentPath;
            if (message.segmentIndex != null && message.hasOwnProperty("segmentIndex"))
                object.segmentIndex = message.segmentIndex;
            if (message.durationSecs != null && message.hasOwnProperty("durationSecs"))
                object.durationSecs = options.json && !isFinite(message.durationSecs) ? String(message.durationSecs) : message.durationSecs;
            if (message.sizeBytes != null && message.hasOwnProperty("sizeBytes"))
                if (typeof message.sizeBytes === "number")
                    object.sizeBytes = options.longs === String ? String(message.sizeBytes) : message.sizeBytes;
                else
                    object.sizeBytes = options.longs === String ? $util.Long.prototype.toString.call(message.sizeBytes) : options.longs === Number ? new $util.LongBits(message.sizeBytes.low >>> 0, message.sizeBytes.high >>> 0).toNumber(true) : message.sizeBytes;
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                object.sessionId = message.sessionId;
            return object;
        };

        /**
         * Converts this SegmentCompleted to JSON.
         * @function toJSON
         * @memberof download_progress.SegmentCompleted
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        SegmentCompleted.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for SegmentCompleted
         * @function getTypeUrl
         * @memberof download_progress.SegmentCompleted
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        SegmentCompleted.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.SegmentCompleted";
        };

        return SegmentCompleted;
    })();

    download_progress.DownloadCompleted = (function() {

        /**
         * Properties of a DownloadCompleted.
         * @memberof download_progress
         * @interface IDownloadCompleted
         * @property {string|null} [downloadId] DownloadCompleted downloadId
         * @property {string|null} [streamerId] DownloadCompleted streamerId
         * @property {string|null} [sessionId] DownloadCompleted sessionId
         * @property {number|Long|null} [totalBytes] DownloadCompleted totalBytes
         * @property {number|null} [totalDurationSecs] DownloadCompleted totalDurationSecs
         * @property {number|null} [totalSegments] DownloadCompleted totalSegments
         */

        /**
         * Constructs a new DownloadCompleted.
         * @memberof download_progress
         * @classdesc Represents a DownloadCompleted.
         * @implements IDownloadCompleted
         * @constructor
         * @param {download_progress.IDownloadCompleted=} [properties] Properties to set
         */
        function DownloadCompleted(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * DownloadCompleted downloadId.
         * @member {string} downloadId
         * @memberof download_progress.DownloadCompleted
         * @instance
         */
        DownloadCompleted.prototype.downloadId = "";

        /**
         * DownloadCompleted streamerId.
         * @member {string} streamerId
         * @memberof download_progress.DownloadCompleted
         * @instance
         */
        DownloadCompleted.prototype.streamerId = "";

        /**
         * DownloadCompleted sessionId.
         * @member {string} sessionId
         * @memberof download_progress.DownloadCompleted
         * @instance
         */
        DownloadCompleted.prototype.sessionId = "";

        /**
         * DownloadCompleted totalBytes.
         * @member {number|Long} totalBytes
         * @memberof download_progress.DownloadCompleted
         * @instance
         */
        DownloadCompleted.prototype.totalBytes = $util.Long ? $util.Long.fromBits(0,0,true) : 0;

        /**
         * DownloadCompleted totalDurationSecs.
         * @member {number} totalDurationSecs
         * @memberof download_progress.DownloadCompleted
         * @instance
         */
        DownloadCompleted.prototype.totalDurationSecs = 0;

        /**
         * DownloadCompleted totalSegments.
         * @member {number} totalSegments
         * @memberof download_progress.DownloadCompleted
         * @instance
         */
        DownloadCompleted.prototype.totalSegments = 0;

        /**
         * Creates a new DownloadCompleted instance using the specified properties.
         * @function create
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {download_progress.IDownloadCompleted=} [properties] Properties to set
         * @returns {download_progress.DownloadCompleted} DownloadCompleted instance
         */
        DownloadCompleted.create = function create(properties) {
            return new DownloadCompleted(properties);
        };

        /**
         * Encodes the specified DownloadCompleted message. Does not implicitly {@link download_progress.DownloadCompleted.verify|verify} messages.
         * @function encode
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {download_progress.IDownloadCompleted} message DownloadCompleted message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadCompleted.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.downloadId != null && Object.hasOwnProperty.call(message, "downloadId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.downloadId);
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.streamerId);
            if (message.sessionId != null && Object.hasOwnProperty.call(message, "sessionId"))
                writer.uint32(/* id 3, wireType 2 =*/26).string(message.sessionId);
            if (message.totalBytes != null && Object.hasOwnProperty.call(message, "totalBytes"))
                writer.uint32(/* id 4, wireType 0 =*/32).uint64(message.totalBytes);
            if (message.totalDurationSecs != null && Object.hasOwnProperty.call(message, "totalDurationSecs"))
                writer.uint32(/* id 5, wireType 1 =*/41).double(message.totalDurationSecs);
            if (message.totalSegments != null && Object.hasOwnProperty.call(message, "totalSegments"))
                writer.uint32(/* id 6, wireType 0 =*/48).uint32(message.totalSegments);
            return writer;
        };

        /**
         * Encodes the specified DownloadCompleted message, length delimited. Does not implicitly {@link download_progress.DownloadCompleted.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {download_progress.IDownloadCompleted} message DownloadCompleted message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadCompleted.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a DownloadCompleted message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.DownloadCompleted} DownloadCompleted
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadCompleted.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.DownloadCompleted();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.downloadId = reader.string();
                        break;
                    }
                case 2: {
                        message.streamerId = reader.string();
                        break;
                    }
                case 3: {
                        message.sessionId = reader.string();
                        break;
                    }
                case 4: {
                        message.totalBytes = reader.uint64();
                        break;
                    }
                case 5: {
                        message.totalDurationSecs = reader.double();
                        break;
                    }
                case 6: {
                        message.totalSegments = reader.uint32();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a DownloadCompleted message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.DownloadCompleted} DownloadCompleted
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadCompleted.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a DownloadCompleted message.
         * @function verify
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        DownloadCompleted.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                if (!$util.isString(message.downloadId))
                    return "downloadId: string expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                if (!$util.isString(message.sessionId))
                    return "sessionId: string expected";
            if (message.totalBytes != null && message.hasOwnProperty("totalBytes"))
                if (!$util.isInteger(message.totalBytes) && !(message.totalBytes && $util.isInteger(message.totalBytes.low) && $util.isInteger(message.totalBytes.high)))
                    return "totalBytes: integer|Long expected";
            if (message.totalDurationSecs != null && message.hasOwnProperty("totalDurationSecs"))
                if (typeof message.totalDurationSecs !== "number")
                    return "totalDurationSecs: number expected";
            if (message.totalSegments != null && message.hasOwnProperty("totalSegments"))
                if (!$util.isInteger(message.totalSegments))
                    return "totalSegments: integer expected";
            return null;
        };

        /**
         * Creates a DownloadCompleted message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.DownloadCompleted} DownloadCompleted
         */
        DownloadCompleted.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.DownloadCompleted)
                return object;
            let message = new $root.download_progress.DownloadCompleted();
            if (object.downloadId != null)
                message.downloadId = String(object.downloadId);
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            if (object.sessionId != null)
                message.sessionId = String(object.sessionId);
            if (object.totalBytes != null)
                if ($util.Long)
                    (message.totalBytes = $util.Long.fromValue(object.totalBytes)).unsigned = true;
                else if (typeof object.totalBytes === "string")
                    message.totalBytes = parseInt(object.totalBytes, 10);
                else if (typeof object.totalBytes === "number")
                    message.totalBytes = object.totalBytes;
                else if (typeof object.totalBytes === "object")
                    message.totalBytes = new $util.LongBits(object.totalBytes.low >>> 0, object.totalBytes.high >>> 0).toNumber(true);
            if (object.totalDurationSecs != null)
                message.totalDurationSecs = Number(object.totalDurationSecs);
            if (object.totalSegments != null)
                message.totalSegments = object.totalSegments >>> 0;
            return message;
        };

        /**
         * Creates a plain object from a DownloadCompleted message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {download_progress.DownloadCompleted} message DownloadCompleted
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        DownloadCompleted.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.downloadId = "";
                object.streamerId = "";
                object.sessionId = "";
                if ($util.Long) {
                    let long = new $util.Long(0, 0, true);
                    object.totalBytes = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                } else
                    object.totalBytes = options.longs === String ? "0" : 0;
                object.totalDurationSecs = 0;
                object.totalSegments = 0;
            }
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                object.downloadId = message.downloadId;
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                object.sessionId = message.sessionId;
            if (message.totalBytes != null && message.hasOwnProperty("totalBytes"))
                if (typeof message.totalBytes === "number")
                    object.totalBytes = options.longs === String ? String(message.totalBytes) : message.totalBytes;
                else
                    object.totalBytes = options.longs === String ? $util.Long.prototype.toString.call(message.totalBytes) : options.longs === Number ? new $util.LongBits(message.totalBytes.low >>> 0, message.totalBytes.high >>> 0).toNumber(true) : message.totalBytes;
            if (message.totalDurationSecs != null && message.hasOwnProperty("totalDurationSecs"))
                object.totalDurationSecs = options.json && !isFinite(message.totalDurationSecs) ? String(message.totalDurationSecs) : message.totalDurationSecs;
            if (message.totalSegments != null && message.hasOwnProperty("totalSegments"))
                object.totalSegments = message.totalSegments;
            return object;
        };

        /**
         * Converts this DownloadCompleted to JSON.
         * @function toJSON
         * @memberof download_progress.DownloadCompleted
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        DownloadCompleted.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for DownloadCompleted
         * @function getTypeUrl
         * @memberof download_progress.DownloadCompleted
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        DownloadCompleted.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.DownloadCompleted";
        };

        return DownloadCompleted;
    })();

    download_progress.DownloadFailed = (function() {

        /**
         * Properties of a DownloadFailed.
         * @memberof download_progress
         * @interface IDownloadFailed
         * @property {string|null} [downloadId] DownloadFailed downloadId
         * @property {string|null} [streamerId] DownloadFailed streamerId
         * @property {string|null} [sessionId] DownloadFailed sessionId
         * @property {string|null} [error] DownloadFailed error
         * @property {boolean|null} [recoverable] DownloadFailed recoverable
         */

        /**
         * Constructs a new DownloadFailed.
         * @memberof download_progress
         * @classdesc Represents a DownloadFailed.
         * @implements IDownloadFailed
         * @constructor
         * @param {download_progress.IDownloadFailed=} [properties] Properties to set
         */
        function DownloadFailed(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * DownloadFailed downloadId.
         * @member {string} downloadId
         * @memberof download_progress.DownloadFailed
         * @instance
         */
        DownloadFailed.prototype.downloadId = "";

        /**
         * DownloadFailed streamerId.
         * @member {string} streamerId
         * @memberof download_progress.DownloadFailed
         * @instance
         */
        DownloadFailed.prototype.streamerId = "";

        /**
         * DownloadFailed sessionId.
         * @member {string} sessionId
         * @memberof download_progress.DownloadFailed
         * @instance
         */
        DownloadFailed.prototype.sessionId = "";

        /**
         * DownloadFailed error.
         * @member {string} error
         * @memberof download_progress.DownloadFailed
         * @instance
         */
        DownloadFailed.prototype.error = "";

        /**
         * DownloadFailed recoverable.
         * @member {boolean} recoverable
         * @memberof download_progress.DownloadFailed
         * @instance
         */
        DownloadFailed.prototype.recoverable = false;

        /**
         * Creates a new DownloadFailed instance using the specified properties.
         * @function create
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {download_progress.IDownloadFailed=} [properties] Properties to set
         * @returns {download_progress.DownloadFailed} DownloadFailed instance
         */
        DownloadFailed.create = function create(properties) {
            return new DownloadFailed(properties);
        };

        /**
         * Encodes the specified DownloadFailed message. Does not implicitly {@link download_progress.DownloadFailed.verify|verify} messages.
         * @function encode
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {download_progress.IDownloadFailed} message DownloadFailed message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadFailed.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.downloadId != null && Object.hasOwnProperty.call(message, "downloadId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.downloadId);
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.streamerId);
            if (message.sessionId != null && Object.hasOwnProperty.call(message, "sessionId"))
                writer.uint32(/* id 3, wireType 2 =*/26).string(message.sessionId);
            if (message.error != null && Object.hasOwnProperty.call(message, "error"))
                writer.uint32(/* id 4, wireType 2 =*/34).string(message.error);
            if (message.recoverable != null && Object.hasOwnProperty.call(message, "recoverable"))
                writer.uint32(/* id 5, wireType 0 =*/40).bool(message.recoverable);
            return writer;
        };

        /**
         * Encodes the specified DownloadFailed message, length delimited. Does not implicitly {@link download_progress.DownloadFailed.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {download_progress.IDownloadFailed} message DownloadFailed message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadFailed.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a DownloadFailed message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.DownloadFailed} DownloadFailed
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadFailed.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.DownloadFailed();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.downloadId = reader.string();
                        break;
                    }
                case 2: {
                        message.streamerId = reader.string();
                        break;
                    }
                case 3: {
                        message.sessionId = reader.string();
                        break;
                    }
                case 4: {
                        message.error = reader.string();
                        break;
                    }
                case 5: {
                        message.recoverable = reader.bool();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a DownloadFailed message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.DownloadFailed} DownloadFailed
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadFailed.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a DownloadFailed message.
         * @function verify
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        DownloadFailed.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                if (!$util.isString(message.downloadId))
                    return "downloadId: string expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                if (!$util.isString(message.sessionId))
                    return "sessionId: string expected";
            if (message.error != null && message.hasOwnProperty("error"))
                if (!$util.isString(message.error))
                    return "error: string expected";
            if (message.recoverable != null && message.hasOwnProperty("recoverable"))
                if (typeof message.recoverable !== "boolean")
                    return "recoverable: boolean expected";
            return null;
        };

        /**
         * Creates a DownloadFailed message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.DownloadFailed} DownloadFailed
         */
        DownloadFailed.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.DownloadFailed)
                return object;
            let message = new $root.download_progress.DownloadFailed();
            if (object.downloadId != null)
                message.downloadId = String(object.downloadId);
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            if (object.sessionId != null)
                message.sessionId = String(object.sessionId);
            if (object.error != null)
                message.error = String(object.error);
            if (object.recoverable != null)
                message.recoverable = Boolean(object.recoverable);
            return message;
        };

        /**
         * Creates a plain object from a DownloadFailed message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {download_progress.DownloadFailed} message DownloadFailed
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        DownloadFailed.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.downloadId = "";
                object.streamerId = "";
                object.sessionId = "";
                object.error = "";
                object.recoverable = false;
            }
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                object.downloadId = message.downloadId;
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                object.sessionId = message.sessionId;
            if (message.error != null && message.hasOwnProperty("error"))
                object.error = message.error;
            if (message.recoverable != null && message.hasOwnProperty("recoverable"))
                object.recoverable = message.recoverable;
            return object;
        };

        /**
         * Converts this DownloadFailed to JSON.
         * @function toJSON
         * @memberof download_progress.DownloadFailed
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        DownloadFailed.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for DownloadFailed
         * @function getTypeUrl
         * @memberof download_progress.DownloadFailed
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        DownloadFailed.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.DownloadFailed";
        };

        return DownloadFailed;
    })();

    download_progress.DownloadCancelled = (function() {

        /**
         * Properties of a DownloadCancelled.
         * @memberof download_progress
         * @interface IDownloadCancelled
         * @property {string|null} [downloadId] DownloadCancelled downloadId
         * @property {string|null} [streamerId] DownloadCancelled streamerId
         * @property {string|null} [sessionId] DownloadCancelled sessionId
         * @property {string|null} [cause] DownloadCancelled cause
         */

        /**
         * Constructs a new DownloadCancelled.
         * @memberof download_progress
         * @classdesc Represents a DownloadCancelled.
         * @implements IDownloadCancelled
         * @constructor
         * @param {download_progress.IDownloadCancelled=} [properties] Properties to set
         */
        function DownloadCancelled(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * DownloadCancelled downloadId.
         * @member {string} downloadId
         * @memberof download_progress.DownloadCancelled
         * @instance
         */
        DownloadCancelled.prototype.downloadId = "";

        /**
         * DownloadCancelled streamerId.
         * @member {string} streamerId
         * @memberof download_progress.DownloadCancelled
         * @instance
         */
        DownloadCancelled.prototype.streamerId = "";

        /**
         * DownloadCancelled sessionId.
         * @member {string} sessionId
         * @memberof download_progress.DownloadCancelled
         * @instance
         */
        DownloadCancelled.prototype.sessionId = "";

        /**
         * DownloadCancelled cause.
         * @member {string} cause
         * @memberof download_progress.DownloadCancelled
         * @instance
         */
        DownloadCancelled.prototype.cause = "";

        /**
         * Creates a new DownloadCancelled instance using the specified properties.
         * @function create
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {download_progress.IDownloadCancelled=} [properties] Properties to set
         * @returns {download_progress.DownloadCancelled} DownloadCancelled instance
         */
        DownloadCancelled.create = function create(properties) {
            return new DownloadCancelled(properties);
        };

        /**
         * Encodes the specified DownloadCancelled message. Does not implicitly {@link download_progress.DownloadCancelled.verify|verify} messages.
         * @function encode
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {download_progress.IDownloadCancelled} message DownloadCancelled message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadCancelled.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.downloadId != null && Object.hasOwnProperty.call(message, "downloadId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.downloadId);
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.streamerId);
            if (message.sessionId != null && Object.hasOwnProperty.call(message, "sessionId"))
                writer.uint32(/* id 3, wireType 2 =*/26).string(message.sessionId);
            if (message.cause != null && Object.hasOwnProperty.call(message, "cause"))
                writer.uint32(/* id 4, wireType 2 =*/34).string(message.cause);
            return writer;
        };

        /**
         * Encodes the specified DownloadCancelled message, length delimited. Does not implicitly {@link download_progress.DownloadCancelled.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {download_progress.IDownloadCancelled} message DownloadCancelled message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadCancelled.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a DownloadCancelled message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.DownloadCancelled} DownloadCancelled
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadCancelled.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.DownloadCancelled();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.downloadId = reader.string();
                        break;
                    }
                case 2: {
                        message.streamerId = reader.string();
                        break;
                    }
                case 3: {
                        message.sessionId = reader.string();
                        break;
                    }
                case 4: {
                        message.cause = reader.string();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a DownloadCancelled message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.DownloadCancelled} DownloadCancelled
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadCancelled.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a DownloadCancelled message.
         * @function verify
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        DownloadCancelled.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                if (!$util.isString(message.downloadId))
                    return "downloadId: string expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                if (!$util.isString(message.sessionId))
                    return "sessionId: string expected";
            if (message.cause != null && message.hasOwnProperty("cause"))
                if (!$util.isString(message.cause))
                    return "cause: string expected";
            return null;
        };

        /**
         * Creates a DownloadCancelled message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.DownloadCancelled} DownloadCancelled
         */
        DownloadCancelled.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.DownloadCancelled)
                return object;
            let message = new $root.download_progress.DownloadCancelled();
            if (object.downloadId != null)
                message.downloadId = String(object.downloadId);
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            if (object.sessionId != null)
                message.sessionId = String(object.sessionId);
            if (object.cause != null)
                message.cause = String(object.cause);
            return message;
        };

        /**
         * Creates a plain object from a DownloadCancelled message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {download_progress.DownloadCancelled} message DownloadCancelled
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        DownloadCancelled.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.downloadId = "";
                object.streamerId = "";
                object.sessionId = "";
                object.cause = "";
            }
            if (message.downloadId != null && message.hasOwnProperty("downloadId"))
                object.downloadId = message.downloadId;
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                object.sessionId = message.sessionId;
            if (message.cause != null && message.hasOwnProperty("cause"))
                object.cause = message.cause;
            return object;
        };

        /**
         * Converts this DownloadCancelled to JSON.
         * @function toJSON
         * @memberof download_progress.DownloadCancelled
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        DownloadCancelled.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for DownloadCancelled
         * @function getTypeUrl
         * @memberof download_progress.DownloadCancelled
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        DownloadCancelled.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.DownloadCancelled";
        };

        return DownloadCancelled;
    })();

    download_progress.DownloadRejected = (function() {

        /**
         * Properties of a DownloadRejected.
         * @memberof download_progress
         * @interface IDownloadRejected
         * @property {string|null} [streamerId] DownloadRejected streamerId
         * @property {string|null} [sessionId] DownloadRejected sessionId
         * @property {string|null} [reason] DownloadRejected reason
         * @property {number|Long|null} [retryAfterSecs] DownloadRejected retryAfterSecs
         * @property {boolean|null} [recoverable] DownloadRejected recoverable
         */

        /**
         * Constructs a new DownloadRejected.
         * @memberof download_progress
         * @classdesc Represents a DownloadRejected.
         * @implements IDownloadRejected
         * @constructor
         * @param {download_progress.IDownloadRejected=} [properties] Properties to set
         */
        function DownloadRejected(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * DownloadRejected streamerId.
         * @member {string} streamerId
         * @memberof download_progress.DownloadRejected
         * @instance
         */
        DownloadRejected.prototype.streamerId = "";

        /**
         * DownloadRejected sessionId.
         * @member {string} sessionId
         * @memberof download_progress.DownloadRejected
         * @instance
         */
        DownloadRejected.prototype.sessionId = "";

        /**
         * DownloadRejected reason.
         * @member {string} reason
         * @memberof download_progress.DownloadRejected
         * @instance
         */
        DownloadRejected.prototype.reason = "";

        /**
         * DownloadRejected retryAfterSecs.
         * @member {number|Long} retryAfterSecs
         * @memberof download_progress.DownloadRejected
         * @instance
         */
        DownloadRejected.prototype.retryAfterSecs = $util.Long ? $util.Long.fromBits(0,0,true) : 0;

        /**
         * DownloadRejected recoverable.
         * @member {boolean} recoverable
         * @memberof download_progress.DownloadRejected
         * @instance
         */
        DownloadRejected.prototype.recoverable = false;

        /**
         * Creates a new DownloadRejected instance using the specified properties.
         * @function create
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {download_progress.IDownloadRejected=} [properties] Properties to set
         * @returns {download_progress.DownloadRejected} DownloadRejected instance
         */
        DownloadRejected.create = function create(properties) {
            return new DownloadRejected(properties);
        };

        /**
         * Encodes the specified DownloadRejected message. Does not implicitly {@link download_progress.DownloadRejected.verify|verify} messages.
         * @function encode
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {download_progress.IDownloadRejected} message DownloadRejected message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadRejected.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.streamerId != null && Object.hasOwnProperty.call(message, "streamerId"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.streamerId);
            if (message.sessionId != null && Object.hasOwnProperty.call(message, "sessionId"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.sessionId);
            if (message.reason != null && Object.hasOwnProperty.call(message, "reason"))
                writer.uint32(/* id 3, wireType 2 =*/26).string(message.reason);
            if (message.retryAfterSecs != null && Object.hasOwnProperty.call(message, "retryAfterSecs"))
                writer.uint32(/* id 4, wireType 0 =*/32).uint64(message.retryAfterSecs);
            if (message.recoverable != null && Object.hasOwnProperty.call(message, "recoverable"))
                writer.uint32(/* id 5, wireType 0 =*/40).bool(message.recoverable);
            return writer;
        };

        /**
         * Encodes the specified DownloadRejected message, length delimited. Does not implicitly {@link download_progress.DownloadRejected.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {download_progress.IDownloadRejected} message DownloadRejected message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        DownloadRejected.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes a DownloadRejected message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.DownloadRejected} DownloadRejected
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadRejected.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.DownloadRejected();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.streamerId = reader.string();
                        break;
                    }
                case 2: {
                        message.sessionId = reader.string();
                        break;
                    }
                case 3: {
                        message.reason = reader.string();
                        break;
                    }
                case 4: {
                        message.retryAfterSecs = reader.uint64();
                        break;
                    }
                case 5: {
                        message.recoverable = reader.bool();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes a DownloadRejected message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.DownloadRejected} DownloadRejected
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        DownloadRejected.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies a DownloadRejected message.
         * @function verify
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        DownloadRejected.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                if (!$util.isString(message.streamerId))
                    return "streamerId: string expected";
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                if (!$util.isString(message.sessionId))
                    return "sessionId: string expected";
            if (message.reason != null && message.hasOwnProperty("reason"))
                if (!$util.isString(message.reason))
                    return "reason: string expected";
            if (message.retryAfterSecs != null && message.hasOwnProperty("retryAfterSecs"))
                if (!$util.isInteger(message.retryAfterSecs) && !(message.retryAfterSecs && $util.isInteger(message.retryAfterSecs.low) && $util.isInteger(message.retryAfterSecs.high)))
                    return "retryAfterSecs: integer|Long expected";
            if (message.recoverable != null && message.hasOwnProperty("recoverable"))
                if (typeof message.recoverable !== "boolean")
                    return "recoverable: boolean expected";
            return null;
        };

        /**
         * Creates a DownloadRejected message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.DownloadRejected} DownloadRejected
         */
        DownloadRejected.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.DownloadRejected)
                return object;
            let message = new $root.download_progress.DownloadRejected();
            if (object.streamerId != null)
                message.streamerId = String(object.streamerId);
            if (object.sessionId != null)
                message.sessionId = String(object.sessionId);
            if (object.reason != null)
                message.reason = String(object.reason);
            if (object.retryAfterSecs != null)
                if ($util.Long)
                    (message.retryAfterSecs = $util.Long.fromValue(object.retryAfterSecs)).unsigned = true;
                else if (typeof object.retryAfterSecs === "string")
                    message.retryAfterSecs = parseInt(object.retryAfterSecs, 10);
                else if (typeof object.retryAfterSecs === "number")
                    message.retryAfterSecs = object.retryAfterSecs;
                else if (typeof object.retryAfterSecs === "object")
                    message.retryAfterSecs = new $util.LongBits(object.retryAfterSecs.low >>> 0, object.retryAfterSecs.high >>> 0).toNumber(true);
            if (object.recoverable != null)
                message.recoverable = Boolean(object.recoverable);
            return message;
        };

        /**
         * Creates a plain object from a DownloadRejected message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {download_progress.DownloadRejected} message DownloadRejected
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        DownloadRejected.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.streamerId = "";
                object.sessionId = "";
                object.reason = "";
                if ($util.Long) {
                    let long = new $util.Long(0, 0, true);
                    object.retryAfterSecs = options.longs === String ? long.toString() : options.longs === Number ? long.toNumber() : long;
                } else
                    object.retryAfterSecs = options.longs === String ? "0" : 0;
                object.recoverable = false;
            }
            if (message.streamerId != null && message.hasOwnProperty("streamerId"))
                object.streamerId = message.streamerId;
            if (message.sessionId != null && message.hasOwnProperty("sessionId"))
                object.sessionId = message.sessionId;
            if (message.reason != null && message.hasOwnProperty("reason"))
                object.reason = message.reason;
            if (message.retryAfterSecs != null && message.hasOwnProperty("retryAfterSecs"))
                if (typeof message.retryAfterSecs === "number")
                    object.retryAfterSecs = options.longs === String ? String(message.retryAfterSecs) : message.retryAfterSecs;
                else
                    object.retryAfterSecs = options.longs === String ? $util.Long.prototype.toString.call(message.retryAfterSecs) : options.longs === Number ? new $util.LongBits(message.retryAfterSecs.low >>> 0, message.retryAfterSecs.high >>> 0).toNumber(true) : message.retryAfterSecs;
            if (message.recoverable != null && message.hasOwnProperty("recoverable"))
                object.recoverable = message.recoverable;
            return object;
        };

        /**
         * Converts this DownloadRejected to JSON.
         * @function toJSON
         * @memberof download_progress.DownloadRejected
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        DownloadRejected.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for DownloadRejected
         * @function getTypeUrl
         * @memberof download_progress.DownloadRejected
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        DownloadRejected.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.DownloadRejected";
        };

        return DownloadRejected;
    })();

    download_progress.ErrorPayload = (function() {

        /**
         * Properties of an ErrorPayload.
         * @memberof download_progress
         * @interface IErrorPayload
         * @property {string|null} [code] ErrorPayload code
         * @property {string|null} [message] ErrorPayload message
         */

        /**
         * Constructs a new ErrorPayload.
         * @memberof download_progress
         * @classdesc Represents an ErrorPayload.
         * @implements IErrorPayload
         * @constructor
         * @param {download_progress.IErrorPayload=} [properties] Properties to set
         */
        function ErrorPayload(properties) {
            if (properties)
                for (let keys = Object.keys(properties), i = 0; i < keys.length; ++i)
                    if (properties[keys[i]] != null)
                        this[keys[i]] = properties[keys[i]];
        }

        /**
         * ErrorPayload code.
         * @member {string} code
         * @memberof download_progress.ErrorPayload
         * @instance
         */
        ErrorPayload.prototype.code = "";

        /**
         * ErrorPayload message.
         * @member {string} message
         * @memberof download_progress.ErrorPayload
         * @instance
         */
        ErrorPayload.prototype.message = "";

        /**
         * Creates a new ErrorPayload instance using the specified properties.
         * @function create
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {download_progress.IErrorPayload=} [properties] Properties to set
         * @returns {download_progress.ErrorPayload} ErrorPayload instance
         */
        ErrorPayload.create = function create(properties) {
            return new ErrorPayload(properties);
        };

        /**
         * Encodes the specified ErrorPayload message. Does not implicitly {@link download_progress.ErrorPayload.verify|verify} messages.
         * @function encode
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {download_progress.IErrorPayload} message ErrorPayload message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        ErrorPayload.encode = function encode(message, writer) {
            if (!writer)
                writer = $Writer.create();
            if (message.code != null && Object.hasOwnProperty.call(message, "code"))
                writer.uint32(/* id 1, wireType 2 =*/10).string(message.code);
            if (message.message != null && Object.hasOwnProperty.call(message, "message"))
                writer.uint32(/* id 2, wireType 2 =*/18).string(message.message);
            return writer;
        };

        /**
         * Encodes the specified ErrorPayload message, length delimited. Does not implicitly {@link download_progress.ErrorPayload.verify|verify} messages.
         * @function encodeDelimited
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {download_progress.IErrorPayload} message ErrorPayload message or plain object to encode
         * @param {$protobuf.Writer} [writer] Writer to encode to
         * @returns {$protobuf.Writer} Writer
         */
        ErrorPayload.encodeDelimited = function encodeDelimited(message, writer) {
            return this.encode(message, writer).ldelim();
        };

        /**
         * Decodes an ErrorPayload message from the specified reader or buffer.
         * @function decode
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @param {number} [length] Message length if known beforehand
         * @returns {download_progress.ErrorPayload} ErrorPayload
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        ErrorPayload.decode = function decode(reader, length, error) {
            if (!(reader instanceof $Reader))
                reader = $Reader.create(reader);
            let end = length === undefined ? reader.len : reader.pos + length, message = new $root.download_progress.ErrorPayload();
            while (reader.pos < end) {
                let tag = reader.uint32();
                if (tag === error)
                    break;
                switch (tag >>> 3) {
                case 1: {
                        message.code = reader.string();
                        break;
                    }
                case 2: {
                        message.message = reader.string();
                        break;
                    }
                default:
                    reader.skipType(tag & 7);
                    break;
                }
            }
            return message;
        };

        /**
         * Decodes an ErrorPayload message from the specified reader or buffer, length delimited.
         * @function decodeDelimited
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {$protobuf.Reader|Uint8Array} reader Reader or buffer to decode from
         * @returns {download_progress.ErrorPayload} ErrorPayload
         * @throws {Error} If the payload is not a reader or valid buffer
         * @throws {$protobuf.util.ProtocolError} If required fields are missing
         */
        ErrorPayload.decodeDelimited = function decodeDelimited(reader) {
            if (!(reader instanceof $Reader))
                reader = new $Reader(reader);
            return this.decode(reader, reader.uint32());
        };

        /**
         * Verifies an ErrorPayload message.
         * @function verify
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {Object.<string,*>} message Plain object to verify
         * @returns {string|null} `null` if valid, otherwise the reason why it is not
         */
        ErrorPayload.verify = function verify(message) {
            if (typeof message !== "object" || message === null)
                return "object expected";
            if (message.code != null && message.hasOwnProperty("code"))
                if (!$util.isString(message.code))
                    return "code: string expected";
            if (message.message != null && message.hasOwnProperty("message"))
                if (!$util.isString(message.message))
                    return "message: string expected";
            return null;
        };

        /**
         * Creates an ErrorPayload message from a plain object. Also converts values to their respective internal types.
         * @function fromObject
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {Object.<string,*>} object Plain object
         * @returns {download_progress.ErrorPayload} ErrorPayload
         */
        ErrorPayload.fromObject = function fromObject(object) {
            if (object instanceof $root.download_progress.ErrorPayload)
                return object;
            let message = new $root.download_progress.ErrorPayload();
            if (object.code != null)
                message.code = String(object.code);
            if (object.message != null)
                message.message = String(object.message);
            return message;
        };

        /**
         * Creates a plain object from an ErrorPayload message. Also converts values to other types if specified.
         * @function toObject
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {download_progress.ErrorPayload} message ErrorPayload
         * @param {$protobuf.IConversionOptions} [options] Conversion options
         * @returns {Object.<string,*>} Plain object
         */
        ErrorPayload.toObject = function toObject(message, options) {
            if (!options)
                options = {};
            let object = {};
            if (options.defaults) {
                object.code = "";
                object.message = "";
            }
            if (message.code != null && message.hasOwnProperty("code"))
                object.code = message.code;
            if (message.message != null && message.hasOwnProperty("message"))
                object.message = message.message;
            return object;
        };

        /**
         * Converts this ErrorPayload to JSON.
         * @function toJSON
         * @memberof download_progress.ErrorPayload
         * @instance
         * @returns {Object.<string,*>} JSON object
         */
        ErrorPayload.prototype.toJSON = function toJSON() {
            return this.constructor.toObject(this, $protobuf.util.toJSONOptions);
        };

        /**
         * Gets the default type url for ErrorPayload
         * @function getTypeUrl
         * @memberof download_progress.ErrorPayload
         * @static
         * @param {string} [typeUrlPrefix] your custom typeUrlPrefix(default "type.googleapis.com")
         * @returns {string} The default type url
         */
        ErrorPayload.getTypeUrl = function getTypeUrl(typeUrlPrefix) {
            if (typeUrlPrefix === undefined) {
                typeUrlPrefix = "type.googleapis.com";
            }
            return typeUrlPrefix + "/download_progress.ErrorPayload";
        };

        return ErrorPayload;
    })();

    return download_progress;
})();

export { $root as default };
